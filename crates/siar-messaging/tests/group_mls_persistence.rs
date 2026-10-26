//! MLS groups through the REAL `GroupService`: real stoolap, real iroh
//! over loopback, real SQLite-backed MLS state — then each side is
//! "restarted" (a brand-new `GroupService` over the same directory and
//! repositories) and must carry on. Nothing in `GroupService`'s MLS path
//! had an end-to-end test before this file; it also covers create → add →
//! welcome → join, which had never been exercised at all.

use std::sync::Arc;
use std::time::Duration;

use siar_crypto::DeviceIdentity;
use siar_domain::{AccountId, ConversationId, DeviceId, MessageContent, MessageText};
use siar_messaging::{
    DeviceDirectory, GroupService, InMemoryDeviceDirectory, InMemoryKeyPackageDirectory,
    KeyPackageDirectory, MemberDevice, PeerTicket,
};
use siar_protocol::{v1::EnvelopeKind, WireMessage};
use siar_storage::{
    open_in_memory, GroupRepository, MessageRepository, StoolapGroupRepository,
    StoolapMessageRepository,
};
use siar_transport::{BlobStore, SiarEndpoint};
use tokio::sync::mpsc;

struct Node {
    account: AccountId,
    device: DeviceId,
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    ticket: PeerTicket,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    groups: Arc<dyn GroupRepository + Send + Sync>,
    directory: Arc<InMemoryDeviceDirectory>,
    mls_dir: std::path::PathBuf,
    incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
}

impl Node {
    async fn spawn(root: &std::path::Path, name: &str) -> Self {
        let (account, device) = (AccountId::new(), DeviceId::new());
        let identity = DeviceIdentity::generate();
        let db = open_in_memory().unwrap();
        let messages: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoolapMessageRepository::new(db.clone()));
        let groups: Arc<dyn GroupRepository + Send + Sync> =
            Arc::new(StoolapGroupRepository::new(db.clone()));
        let blobs: Arc<dyn siar_storage::BlobRepository + Send + Sync> =
            Arc::new(siar_storage::StoolapBlobRepository::new(db));
        let store: Arc<dyn BlobStore> = Arc::new(siar_messaging::StorageBlobStore(blobs));
        let (tx, incoming) = mpsc::channel(32);
        let endpoint = Arc::new(
            SiarEndpoint::bind(iroh::SecretKey::generate(), tx, store)
                .await
                .unwrap(),
        );
        // direct loopback address only (no relay/DNS discovery in a sandbox)
        let full = endpoint.addr();
        let ticket = PeerTicket {
            endpoint_addr: iroh::EndpointAddr::from_parts(
                full.id,
                full.addrs
                    .into_iter()
                    .filter(|a| matches!(a, iroh::TransportAddr::Ip(_))),
            ),
            x25519_public: identity.x25519_public().to_bytes(),
            ed25519_verifying: identity.verifying_key().to_bytes(),
        };
        Self {
            account,
            device,
            identity,
            endpoint,
            ticket,
            messages,
            groups,
            directory: Arc::new(InMemoryDeviceDirectory::new()),
            mls_dir: root.join(name),
            incoming,
        }
    }

    /// A fresh GroupService over this node's persistent MLS directory —
    /// what a restarted app builds.
    fn service(&self) -> GroupService {
        GroupService::new_persistent(
            self.device,
            self.account,
            self.identity.try_clone().unwrap(),
            self.endpoint.clone(),
            self.messages.clone(),
            self.directory.clone(),
            self.groups.clone(),
            &self.mls_dir,
        )
        .expect("service")
    }

    async fn next_envelope(&mut self) -> siar_protocol::v1::Envelope {
        let frame = tokio::time::timeout(Duration::from_secs(20), self.incoming.recv())
            .await
            .expect("timed out waiting for a frame")
            .expect("channel open");
        match frame.message {
            WireMessage::V1(e) => e,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

fn text(s: &str) -> MessageText {
    MessageText::parse(s.to_string()).unwrap()
}

fn plain(c: Option<MessageContent>) -> String {
    match c {
        Some(MessageContent::Text(t)) => t.as_str().to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn groups_survive_a_restart_on_both_sides() {
    let root = std::env::temp_dir().join(format!("siar-grp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut a, mut b) = (Node::spawn(&root, "a").await, Node::spawn(&root, "b").await);
    // each side knows how to reach the other (a contact, in app terms)
    a.directory.register(
        b.account,
        MemberDevice {
            device_id: b.device,
            ticket: b.ticket.clone(),
        },
    );
    b.directory.register(
        a.account,
        MemberDevice {
            device_id: a.device,
            ticket: a.ticket.clone(),
        },
    );

    let (svc_a, svc_b) = (a.service(), b.service());
    let kp_dir = InMemoryKeyPackageDirectory::default();
    svc_b.publish_key_package(&kp_dir).unwrap();
    let kp = kp_dir.take(b.device).expect("B's key package");

    // A founds a group and adds B; B joins from the welcome.
    let conv = ConversationId::new();
    svc_a.create_group_mls(conv, a.account).unwrap();
    svc_a
        .add_member_mls(conv, b.account, b.device, &kp)
        .await
        .unwrap();
    let welcome = b.next_envelope().await;
    assert!(matches!(welcome.kind, EnvelopeKind::GroupMlsWelcome));
    // The invite carries the roster itself: no out-of-band state.
    let joined = svc_b
        .join_group_from_invite(conv, &welcome.payload)
        .unwrap();
    assert_eq!(joined.conversation_id, conv);
    assert!(joined.is_member(a.account) && joined.is_member(b.account));

    // Traffic works before any restart.
    svc_a.send_text_mls(conv, text("before-1")).await.unwrap();
    let env = b.next_envelope().await;
    assert!(matches!(env.kind, EnvelopeKind::GroupMlsApplication));
    assert_eq!(
        plain(svc_b.handle_incoming_mls(conv, &env).unwrap()),
        "before-1"
    );

    // ---- A restarts ----
    drop(svc_a);
    let svc_a = a.service();
    assert_eq!(
        svc_a.restore_mls_sessions(),
        vec![conv],
        "A's group must come back"
    );
    svc_a
        .send_text_mls(conv, text("after-A-restart"))
        .await
        .unwrap();
    let env = b.next_envelope().await;
    assert_eq!(
        plain(svc_b.handle_incoming_mls(conv, &env).unwrap()),
        "after-A-restart"
    );

    // ---- B restarts ----
    drop(svc_b);
    let svc_b = b.service();
    assert_eq!(
        svc_b.restore_mls_sessions(),
        vec![conv],
        "B's joined group must come back"
    );
    svc_b
        .send_text_mls(conv, text("after-B-restart"))
        .await
        .unwrap();
    let env = a.next_envelope().await;
    assert_eq!(
        plain(svc_a.handle_incoming_mls(conv, &env).unwrap()),
        "after-B-restart"
    );

    // and the reloaded A can still decrypt what the reloaded B sent next
    svc_b.send_text_mls(conv, text("again")).await.unwrap();
    let env = a.next_envelope().await;
    assert_eq!(
        plain(svc_a.handle_incoming_mls(conv, &env).unwrap()),
        "again"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_published_key_package_stays_valid_across_a_restart() {
    let root = std::env::temp_dir().join(format!("siar-kp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut a, mut b) = (Node::spawn(&root, "a").await, Node::spawn(&root, "b").await);
    a.directory.register(
        b.account,
        MemberDevice {
            device_id: b.device,
            ticket: b.ticket.clone(),
        },
    );
    b.directory.register(
        a.account,
        MemberDevice {
            device_id: a.device,
            ticket: a.ticket.clone(),
        },
    );

    // B publishes, the package is handed to A out of band... then B restarts.
    let kp_dir = InMemoryKeyPackageDirectory::default();
    b.service().publish_key_package(&kp_dir).unwrap();
    let shared = kp_dir.take(b.device).expect("published");

    let svc_b = b.service(); // restarted app
    let kp_dir2 = InMemoryKeyPackageDirectory::default();
    svc_b.ensure_key_package_published(&kp_dir2).unwrap();
    assert_eq!(
        kp_dir2.take(b.device).unwrap(),
        shared,
        "the SAME key package is reused"
    );

    // A uses the package B shared BEFORE its restart; B can still join with it.
    let svc_a = a.service();
    let conv = ConversationId::new();
    svc_a.create_group_mls(conv, a.account).unwrap();
    svc_a
        .add_member_mls(conv, b.account, b.device, &shared)
        .await
        .unwrap();
    let welcome = b.next_envelope().await;
    svc_b
        .join_group_from_invite(conv, &welcome.payload)
        .expect("joins with the pre-restart package");

    svc_a.send_text_mls(conv, text("hello")).await.unwrap();
    let env = b.next_envelope().await;
    assert_eq!(
        plain(svc_b.handle_incoming_mls(conv, &env).unwrap()),
        "hello"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = &mut a;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invite_for_the_wrong_group_or_account_is_refused() {
    let root = std::env::temp_dir().join(format!("siar-bad-invite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (a, mut b) = (Node::spawn(&root, "a").await, Node::spawn(&root, "b").await);
    a.directory.register(
        b.account,
        MemberDevice {
            device_id: b.device,
            ticket: b.ticket.clone(),
        },
    );
    let (svc_a, svc_b) = (a.service(), b.service());
    let kp_dir = InMemoryKeyPackageDirectory::default();
    svc_b.publish_key_package(&kp_dir).unwrap();
    let kp = kp_dir.take(b.device).unwrap();
    let conv = ConversationId::new();
    svc_a.create_group_mls(conv, a.account).unwrap();
    svc_a
        .add_member_mls(conv, b.account, b.device, &kp)
        .await
        .unwrap();
    let welcome = b.next_envelope().await;

    // an invite delivered under a different conversation id is not adopted
    let other = ConversationId::new();
    assert!(svc_b
        .join_group_from_invite(other, &welcome.payload)
        .is_err());
    // a bare MLS welcome (older client) is not a bundle
    assert!(siar_messaging::WelcomeBundle::decode(&[0u8; 40]).is_none());
    assert!(siar_messaging::WelcomeBundle::decode(&[]).is_none());
    // the right one still works afterwards (the failed attempts consumed nothing)
    svc_b
        .join_group_from_invite(conv, &welcome.payload)
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

async fn key_package_of(node: &Node, svc: &GroupService) -> Vec<u8> {
    let dir = InMemoryKeyPackageDirectory::default();
    svc.publish_key_package(&dir).unwrap();
    dir.take(node.device).expect("published")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_find_each_other_without_being_contacts_and_every_roster_stays_in_sync() {
    let root = std::env::temp_dir().join(format!("siar-disc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut a, mut b, mut c) = (
        Node::spawn(&root, "a").await,
        Node::spawn(&root, "b").await,
        Node::spawn(&root, "c").await,
    );
    // Only the ADMIN knows anyone, and only the people it is about to add.
    // B and C know nobody: no contacts, no tickets exchanged by hand.
    a.directory.register(
        b.account,
        MemberDevice {
            device_id: b.device,
            ticket: b.ticket.clone(),
        },
    );
    a.directory.register(
        c.account,
        MemberDevice {
            device_id: c.device,
            ticket: c.ticket.clone(),
        },
    );
    let (svc_a, svc_b, svc_c) = (a.service(), b.service(), c.service());
    let (kp_b, kp_c) = (
        key_package_of(&b, &svc_b).await,
        key_package_of(&c, &svc_c).await,
    );
    assert!(
        b.directory.devices_for(a.account).is_empty()
            && c.directory.devices_for(a.account).is_empty()
    );

    let conv = ConversationId::new();
    svc_a.create_group_mls(conv, a.account).unwrap();

    // A adds B. B learns how to reach A from the invite itself.
    svc_a
        .add_member_mls(conv, b.account, b.device, &kp_b)
        .await
        .unwrap();
    let welcome_b = b.next_envelope().await;
    svc_b
        .join_group_from_invite(conv, &welcome_b.payload)
        .unwrap();
    assert!(
        !b.directory.devices_for(a.account).is_empty(),
        "B learned A from the invite"
    );

    // A adds C. B — an EXISTING member — must learn about C from the commit:
    // both the roster change and how to reach C.
    svc_a
        .add_member_mls(conv, c.account, c.device, &kp_c)
        .await
        .unwrap();
    let commit = b.next_envelope().await;
    assert!(matches!(commit.kind, EnvelopeKind::GroupMlsCommit));
    assert!(svc_b.handle_incoming_mls(conv, &commit).unwrap().is_none());
    let roster_b = svc_b.group_state(conv).unwrap().unwrap();
    assert!(roster_b.is_member(c.account), "B's own roster now lists C");
    assert_eq!(roster_b.members().len(), 3);
    assert!(
        !b.directory.devices_for(c.account).is_empty(),
        "B can now reach C"
    );

    // C joins and learns A and B — though it never met B.
    let welcome_c = c.next_envelope().await;
    svc_c
        .join_group_from_invite(conv, &welcome_c.payload)
        .unwrap();
    assert!(!c.directory.devices_for(a.account).is_empty());
    assert!(
        !c.directory.devices_for(b.account).is_empty(),
        "C learned B from the invite"
    );

    // B → A and C, with no contact ever saved between B and C.
    svc_b.send_text_mls(conv, text("from b")).await.unwrap();
    let (env_a, env_c) = (a.next_envelope().await, c.next_envelope().await);
    assert_eq!(
        plain(svc_a.handle_incoming_mls(conv, &env_a).unwrap()),
        "from b"
    );
    assert_eq!(
        plain(svc_c.handle_incoming_mls(conv, &env_c).unwrap()),
        "from b"
    );

    // C → A and B.
    svc_c.send_text_mls(conv, text("from c")).await.unwrap();
    let (env_a, env_b) = (a.next_envelope().await, b.next_envelope().await);
    assert_eq!(
        plain(svc_a.handle_incoming_mls(conv, &env_a).unwrap()),
        "from c"
    );
    assert_eq!(
        plain(svc_b.handle_incoming_mls(conv, &env_b).unwrap()),
        "from c"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_add_from_a_device_that_is_not_a_known_admin_cannot_change_a_roster() {
    let root = std::env::temp_dir().join(format!("siar-spoof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (a, mut b, c) = (
        Node::spawn(&root, "a").await,
        Node::spawn(&root, "b").await,
        Node::spawn(&root, "c").await,
    );
    a.directory.register(
        b.account,
        MemberDevice {
            device_id: b.device,
            ticket: b.ticket.clone(),
        },
    );
    a.directory.register(
        c.account,
        MemberDevice {
            device_id: c.device,
            ticket: c.ticket.clone(),
        },
    );
    let (svc_a, svc_b, svc_c) = (a.service(), b.service(), c.service());
    let (kp_b, kp_c) = (
        key_package_of(&b, &svc_b).await,
        key_package_of(&c, &svc_c).await,
    );
    let conv = ConversationId::new();
    svc_a.create_group_mls(conv, a.account).unwrap();
    svc_a
        .add_member_mls(conv, b.account, b.device, &kp_b)
        .await
        .unwrap();
    let welcome = b.next_envelope().await;
    svc_b
        .join_group_from_invite(conv, &welcome.payload)
        .unwrap();
    assert_eq!(svc_b.group_state(conv).unwrap().unwrap().members().len(), 2);

    svc_a
        .add_member_mls(conv, c.account, c.device, &kp_c)
        .await
        .unwrap();
    let mut commit = b.next_envelope().await;
    // The very same (cryptographically valid) commit, but claiming to come
    // from a device B has never known as an admin.
    commit.sender = DeviceId::new();
    svc_b.handle_incoming_mls(conv, &commit).unwrap();
    let roster = svc_b.group_state(conv).unwrap().unwrap();
    assert_eq!(
        roster.members().len(),
        2,
        "the roster must not change on an unverified sender's say-so"
    );
    assert!(
        b.directory.devices_for(c.account).is_empty(),
        "and no address may be planted either"
    );
    let _ = std::fs::remove_dir_all(&root);
}
