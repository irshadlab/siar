//! Device linking inside the core: storage, the offer/approve/join
//! operations, and the handler for `WireMessage::Link`. The rules (codes,
//! expiry, approval, certificates) live in [`crate::linking`]; this module
//! connects them to disk, the transport and the device list.
//!
//! **Who can link.** The device that created the account holds the account's
//! root key (`root.key`, created the first time it opens an offer) and is the
//! only one that can approve new devices. A device that was itself linked
//! holds no root key and cannot approve others — sharing the root key between
//! devices is a separate design (it is what the spec's "any full-trust
//! device" policy would need) and is not attempted here.
//!
//! **Joining.** Only a fresh device may join (no paired peers, no groups, not
//! already linked): joining replaces the account id, and a device with
//! history under the old id would orphan it. The new account id is written to
//! `account_id.txt`; the running core keeps the old id until it restarts,
//! which the user is told.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};
use siar_identity_multidevice::certificate::DeviceCertificate;
use siar_identity_multidevice::root_key::{RootIdentityKey, RootPublicKey};
use siar_messaging::PeerTicket;
use siar_protocol::link::LinkMessage;
use siar_protocol::WireMessage;
use siar_ui_model::NoticeLevel;
use siar_ui_state::{DeviceSecurityFlag, DeviceSecurityView, DeviceTrustState};

use crate::core::Core;
use crate::linking::{accept_certificate, LinkError, LinkRequest};

const ROOT_KEY_FILE: &str = "root.key";
const LINKED_FILE: &str = "linked_devices.bin";
const JOINED_FILE: &str = "joined_account.bin";
const ACCOUNT_FILE: &str = "account_id.txt";
/// How long a joining device waits for a person to approve it.
pub const JOIN_WAIT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedDevice {
    pub device_id: DeviceId,
    pub name: String,
    pub public_key: [u8; 32],
    pub added_at_millis: u64,
    /// That device's ticket (how contacts and this device reach it).
    pub ticket: String,
    /// Its certificate (postcard), kept so it can be announced again later;
    /// empty on the joined device's record of the approver.
    pub certificate: Vec<u8>,
}

#[derive(Serialize, Deserialize, Default)]
struct LinkedFile {
    devices: Vec<LinkedDevice>,
}

/// What a joined device remembers about the account it joined.
#[derive(Serialize, Deserialize)]
struct JoinedFile {
    account_id: AccountId,
    root_public_key: [u8; 32],
    certificate: Vec<u8>,
}

// --- storage -------------------------------------------------------------

fn write_private(path: &Path, bytes: &[u8], must_be_new: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if must_be_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600); // owner only: this is key material
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn load_root(dir: &Path) -> Option<RootIdentityKey> {
    let bytes = std::fs::read(dir.join(ROOT_KEY_FILE)).ok()?;
    let seed: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(RootIdentityKey::from_seed(&seed))
}

/// The account's root key, created on first use. `Err` for a joined device,
/// which must never invent a second root for an account it joined.
fn root_key_for_offers(dir: &Path) -> Result<RootIdentityKey, String> {
    if dir.join(JOINED_FILE).exists() {
        return Err(
            "This device joined an existing account; only the device that created it can link new ones."
                .into(),
        );
    }
    if let Some(k) = load_root(dir) {
        return Ok(k);
    }
    let key = RootIdentityKey::generate();
    write_private(&dir.join(ROOT_KEY_FILE), &*key.seed(), true)
        .map_err(|e| format!("couldn’t store the account key: {e}"))?;
    Ok(key)
}

const IMPORTED_FILE: &str = "imported_contacts.bin";

pub(crate) fn load_imported(dir: &Path) -> Vec<String> {
    std::fs::read(dir.join(IMPORTED_FILE))
        .ok()
        .and_then(|b| postcard::from_bytes::<Vec<String>>(&b).ok())
        .unwrap_or_default()
}

fn save_imported(dir: &Path, tickets: &[String]) -> std::io::Result<()> {
    let bytes = postcard::to_allocvec(tickets).expect("strings always serialize");
    let tmp = dir.join(format!("{IMPORTED_FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, dir.join(IMPORTED_FILE))
}

pub(crate) fn load_linked(dir: &Path) -> Vec<LinkedDevice> {
    std::fs::read(dir.join(LINKED_FILE))
        .ok()
        .and_then(|b| postcard::from_bytes::<LinkedFile>(&b).ok())
        .map(|f| f.devices)
        .unwrap_or_default()
}

fn save_linked(dir: &Path, devices: &[LinkedDevice]) -> std::io::Result<()> {
    let bytes = postcard::to_allocvec(&LinkedFile {
        devices: devices.to_vec(),
    })
    .expect("a list of plain structs always serializes");
    let tmp = dir.join(format!("{LINKED_FILE}.tmp"));
    write_private(&tmp, &bytes, false)?;
    std::fs::rename(tmp, dir.join(LINKED_FILE))
}

/// This device first, then every linked one (Trusted, flagged as not yet
/// reviewed since linking).
#[must_use]
pub(crate) fn device_list(
    this: DeviceSecurityView,
    linked: &[LinkedDevice],
) -> Vec<DeviceSecurityView> {
    let mut out = vec![this];
    out.extend(linked.iter().map(|l| DeviceSecurityView {
        id: l.device_id,
        display_name: l.name.clone(),
        kind: siar_ui_state::DeviceKind::Unknown,
        status: DeviceTrustState::Trusted,
        added_at_millis: l.added_at_millis,
        last_active_millis: None,
        current_device: false,
        security_flags: vec![DeviceSecurityFlag::UnreviewedSinceLinking],
    }));
    out
}

// --- what the operations return ----------------------------------------

pub struct LinkOfferView {
    pub code: String,
    pub ticket: String,
}

pub struct PendingLinkView {
    pub device_id: DeviceId,
    pub name: String,
    pub fingerprint: String,
}

fn millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A joining device waiting for the answer to its request.
pub(crate) struct JoinWait {
    pub(crate) approver: iroh::EndpointId,
    pub(crate) approver_ticket: String,
    pub(crate) until: std::time::Instant,
}

impl Core {
    fn this_device_view(&self) -> DeviceSecurityView {
        self.hub
            .read(|s| s.devices.this_device().cloned())
            .unwrap_or_else(|| DeviceSecurityView {
                id: self.my_device,
                display_name: crate::host::device_label(),
                kind: crate::host::device_kind(),
                status: DeviceTrustState::Trusted,
                added_at_millis: 0,
                last_active_millis: None,
                current_device: true,
                security_flags: Vec::new(),
            })
    }

    fn refresh_devices(&self) {
        let list = device_list(self.this_device_view(), &load_linked(&self.data_dir));
        self.hub.update(|s| s.devices.set_devices(list));
    }

    /// Open a one-time link offer. Show `code` and `ticket` to the person
    /// setting up the other device.
    pub fn link_open_offer(&self) -> Result<LinkOfferView, String> {
        root_key_for_offers(&self.data_dir)?; // creates the key on first use
        let code = self
            .link_offers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .new_offer(millis_now());
        let view = LinkOfferView {
            code,
            ticket: self.hub.read(|s| s.identity.ticket.clone()),
        };
        *self
            .link_shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            Some((view.code.clone(), view.ticket.clone()));
        self.refresh_link_state();
        Ok(view)
    }

    /// Bring the window's picture of linking up to date (cheap; the hub only
    /// notifies when something actually changed).
    pub(crate) fn refresh_link_state(&self) {
        let now = millis_now();
        let open = self
            .link_offers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .has_open_offer(now);
        let offer = if open {
            self.link_shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .map(|(code, ticket)| siar_ui_state::LinkOffer { code, ticket })
        } else {
            None
        };
        let pending = self
            .link_pending()
            .into_iter()
            .map(|p| siar_ui_state::PendingLinkRow {
                device_id: p.device_id,
                name: p.name,
                fingerprint: p.fingerprint,
            })
            .collect();
        let next = siar_ui_state::LinkState {
            offer,
            pending,
            restart_needed: self.link_restart_needed(),
        };
        if self.hub.read(|s| s.link != next) {
            self.hub.update(|s| s.link = next);
        }
    }

    #[must_use]
    pub fn link_pending(&self) -> Vec<PendingLinkView> {
        self.link_offers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending(millis_now())
            .into_iter()
            .map(|p| PendingLinkView {
                device_id: p.request.device_id,
                name: p.request.display_name.clone(),
                fingerprint: p.fingerprint.clone(),
            })
            .collect()
    }

    /// A person approved `device`: issue its certificate and send it.
    ///
    /// With `share_contacts`, the new device also receives this device's
    /// contact tickets (so it can reach them without re-pairing). Either way,
    /// every contact is told the account has a new device.
    pub async fn link_approve(&self, device: DeviceId, share_contacts: bool) -> Result<(), String> {
        let root = root_key_for_offers(&self.data_dir)?;
        let now = millis_now();
        let (pending, cert) = {
            let mut offers = self
                .link_offers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let pending = offers
                .pending(now)
                .into_iter()
                .find(|p| p.request.device_id == device)
                .map(|p| (*p).clone())
                .ok_or_else(|| LinkError::NoSuchRequest.to_string())?;
            let cert = offers
                .approve(device, now, &root, self.local_account, 1)
                .map_err(|e| e.to_string())?;
            (pending, cert)
        };
        let endpoint = self
            .link_endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&device)
            .ok_or_else(|| "that device is no longer connected".to_owned())?;
        let mut linked = load_linked(&self.data_dir);
        linked.retain(|l| l.device_id != device);
        let certificate = postcard::to_allocvec(&cert).map_err(|e| e.to_string())?;
        linked.push(LinkedDevice {
            device_id: device,
            name: pending.request.display_name.clone(),
            public_key: pending.request.device_public_key,
            added_at_millis: now,
            ticket: pending.request.ticket.clone(),
            certificate: certificate.clone(),
        });
        save_linked(&self.data_dir, &linked)
            .map_err(|e| format!("couldn’t save the device: {e}"))?;
        self.refresh_devices();
        let msg = LinkMessage::Approved {
            account_id: self.local_account,
            root_public_key: root.root_public_key().0,
            certificate: certificate.clone(),
            approver_device: self.my_device,
            approver_public_key: self.local_key,
            approver_name: self.this_device_view().display_name,
            contact_tickets: if share_contacts {
                self.contact_ticket_texts()
            } else {
                Vec::new()
            },
        };
        self.service
            .send_wire(iroh::EndpointAddr::new(endpoint), &WireMessage::Link(msg))
            .await
            .map_err(|e| format!("the new device couldn’t be reached: {e}"))?;
        // Tell every contact the account has a new device (best effort; it is
        // repeated at start-up for any contact that was offline).
        self.announce_device(
            root.root_public_key().0,
            certificate,
            pending.request.ticket.clone(),
        )
        .await;
        self.refresh_link_state();
        Ok(())
    }

    fn contact_ticket_texts(&self) -> Vec<String> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .all()
            .into_iter()
            .take(siar_protocol::link::MAX_LINK_CONTACTS)
            .map(|(_, t)| t.encode())
            .collect()
    }

    /// Send `DeviceAdded` to every paired contact. Failures are only logged: a
    /// contact that is offline hears it at the next start-up announcement.
    async fn announce_device(
        &self,
        root_public_key: [u8; 32],
        certificate: Vec<u8>,
        ticket: String,
    ) {
        let contacts = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .all();
        for (_, contact) in contacts {
            let msg = LinkMessage::DeviceAdded {
                account_id: self.local_account,
                root_public_key,
                certificate: certificate.clone(),
                device_ticket: ticket.clone(),
            };
            if let Err(e) = self
                .service
                .send_wire(contact.endpoint_addr.clone(), &WireMessage::Link(msg))
                .await
            {
                tracing::debug!(error = %e, "couldn’t announce a new device to a contact");
            }
        }
    }

    /// Re-announce every linked device to every contact (the root key is read
    /// from disk; a device without a root key has nothing to announce).
    pub(crate) async fn announce_linked_devices(&self) {
        let Some(root) = load_root(&self.data_dir) else {
            return;
        };
        for l in load_linked(&self.data_dir) {
            if l.certificate.is_empty() || l.ticket.is_empty() {
                continue;
            }
            self.announce_device(root.root_public_key().0, l.certificate, l.ticket)
                .await;
        }
    }

    /// A person declined `device`.
    pub async fn link_reject(&self, device: DeviceId) -> bool {
        let removed = self
            .link_offers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reject(device);
        let endpoint = self
            .link_endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&device);
        if let Some(e) = endpoint {
            let _ = self
                .service
                .send_wire(
                    iroh::EndpointAddr::new(e),
                    &WireMessage::Link(LinkMessage::Denied),
                )
                .await;
        }
        removed
    }

    /// Ask the device at `ticket_text` to link this device to its account,
    /// using the one-time `code` its owner was shown. A person must then
    /// approve on that device.
    pub async fn link_join(&self, ticket_text: &str, code: &str) -> Result<(), String> {
        if self.data_dir.join(JOINED_FILE).exists() || self.data_dir.join(ROOT_KEY_FILE).exists() {
            return Err("This device already belongs to an account.".into());
        }
        let has_history = !self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .all()
            .is_empty()
            || self.hub.read(|s| s.groups.ordered().len()) > 0;
        if has_history {
            return Err(
                "Only a fresh device can join an account: this one already has contacts or groups."
                    .into(),
            );
        }
        let approver = PeerTicket::decode(ticket_text.trim())
            .map_err(|_| "That link ticket isn’t valid.".to_owned())?;
        *self
            .link_join
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(JoinWait {
            approver: approver.endpoint_addr.id,
            approver_ticket: ticket_text.trim().to_owned(),
            until: std::time::Instant::now() + JOIN_WAIT,
        });
        let msg = LinkMessage::Request {
            code: code.trim().to_owned(),
            device_id: self.my_device,
            device_public_key: self.local_key,
            display_name: self.this_device_view().display_name,
            device_ticket: self.hub.read(|s| s.identity.ticket.clone()),
        };
        self.service
            .send_wire(approver.endpoint_addr.clone(), &WireMessage::Link(msg))
            .await
            .map_err(|e| format!("couldn’t reach that device: {e}"))
    }

    /// Whether this device joined an account and the running core still uses
    /// its old id (a restart finishes the join).
    #[must_use]
    pub fn link_restart_needed(&self) -> bool {
        self.data_dir.join(JOINED_FILE).exists()
            && std::fs::read_to_string(self.data_dir.join(ACCOUNT_FILE))
                .ok()
                .and_then(|t| uuid::Uuid::parse_str(t.trim()).ok())
                .is_some_and(|id| id != self.local_account.as_uuid())
    }

    /// A `WireMessage::Link` arrived from the authenticated endpoint `from`.
    pub(crate) async fn handle_link_message(&self, from: iroh::EndpointId, msg: LinkMessage) {
        if !msg.within_limits() {
            tracing::warn!(?from, "dropping an oversize link message");
            return;
        }
        match msg {
            LinkMessage::Request {
                code,
                device_id,
                device_public_key,
                display_name,
                device_ticket,
            } => {
                let result = self
                    .link_offers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .submit(
                        millis_now(),
                        &code,
                        LinkRequest {
                            device_id,
                            device_public_key,
                            display_name,
                            ticket: device_ticket,
                        },
                    );
                match result {
                    Ok(p) => {
                        self.link_endpoints
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(device_id, from);
                        self.hub.notify(
                            NoticeLevel::Info,
                            format!(
                                "“{}” wants to join your account (key {}). Approve it only if that is your device.",
                                p.request.display_name, p.fingerprint
                            ),
                        );
                    }
                    Err(e) => {
                        // Say nothing about *why* to the requester.
                        tracing::info!(error = %e, "refused a link request");
                        let _ = self
                            .service
                            .send_wire(
                                iroh::EndpointAddr::new(from),
                                &WireMessage::Link(LinkMessage::Denied),
                            )
                            .await;
                    }
                }
            }
            LinkMessage::Denied => {
                if self.take_join_wait(&from).is_some() {
                    self.hub.notify(
                        NoticeLevel::Warning,
                        "The other device did not accept the link request.",
                    );
                }
            }
            LinkMessage::DeviceAdded {
                account_id: _,
                root_public_key,
                certificate,
                device_ticket,
            } => self.handle_device_added(from, root_public_key, &certificate, &device_ticket),
            LinkMessage::Approved {
                account_id,
                root_public_key,
                certificate,
                approver_device,
                approver_public_key,
                approver_name,
                contact_tickets,
            } => {
                let Some(wait) = self.take_join_wait(&from) else {
                    tracing::warn!(?from, "ignoring an unsolicited link approval");
                    return;
                };
                let cert: DeviceCertificate = match postcard::from_bytes(&certificate) {
                    Ok(c) => c,
                    Err(_) => {
                        self.hub
                            .notify(NoticeLevel::Warning, "The link approval was malformed.");
                        return;
                    }
                };
                if cert.account_id != account_id
                    || accept_certificate(
                        &RootPublicKey(root_public_key),
                        &cert,
                        self.my_device,
                        &self.local_key,
                        millis_now(),
                    )
                    .is_err()
                {
                    self.hub.notify(
                        NoticeLevel::Warning,
                        "The link approval was not valid for this device, so it was ignored.",
                    );
                    return;
                }
                if let Err(e) = self.adopt_account(
                    account_id,
                    root_public_key,
                    certificate,
                    LinkedDevice {
                        device_id: approver_device,
                        name: approver_name.chars().filter(|c| !c.is_control()).collect(),
                        public_key: approver_public_key,
                        added_at_millis: millis_now(),
                        ticket: wait.approver_ticket,
                        certificate: Vec::new(),
                    },
                ) {
                    self.hub.notify(NoticeLevel::Warning, e);
                    return;
                }
                let imported = self.import_contacts(&contact_tickets);
                self.refresh_devices();
                self.hub.notify(
                    NoticeLevel::Info,
                    if imported > 0 {
                        format!("This device is linked and added {imported} contact(s). Restart Siar to finish joining the account.")
                    } else {
                        "This device is linked. Restart Siar to finish joining the account.".to_owned()
                    },
                );
            }
        }
        self.refresh_link_state();
    }

    /// Consume the join wait if `from` is the device we asked and it has not
    /// expired.
    fn take_join_wait(&self, from: &iroh::EndpointId) -> Option<JoinWait> {
        let mut wait = self
            .link_join
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let ok = wait
            .as_ref()
            .is_some_and(|w| w.approver == *from && w.until > std::time::Instant::now());
        if ok {
            wait.take()
        } else {
            None
        }
    }

    /// A paired contact says its account has a new device.
    fn handle_device_added(
        &self,
        from: iroh::EndpointId,
        root_public_key: [u8; 32],
        certificate: &[u8],
        device_ticket: &str,
    ) {
        // Only an already-paired contact may announce devices for itself: the
        // authenticated connection is what ties the announcement to a person
        // you chose to trust.
        let Some(contact) = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .conversation_of(&from)
        else {
            tracing::warn!(
                ?from,
                "ignoring a device announcement from an unpaired device"
            );
            return;
        };
        let (Ok(cert), Ok(ticket)) = (
            postcard::from_bytes::<DeviceCertificate>(certificate),
            PeerTicket::decode(device_ticket),
        ) else {
            tracing::warn!(?from, "ignoring a malformed device announcement");
            return;
        };
        let mut devices = self
            .contact_devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(why) = crate::contact_devices::verify_announcement(
            devices.known_root(contact),
            root_public_key,
            &cert,
            &ticket,
            millis_now(),
        ) {
            drop(devices);
            let label = self.contact_label(contact);
            self.hub.notify(
                NoticeLevel::Warning,
                match why {
                    crate::contact_devices::AnnounceError::RootChanged => format!(
                        "{label}’s account key changed. A new device they announced was NOT added; verify them again."
                    ),
                    _ => format!("A new device announced by {label} could not be verified and was ignored."),
                },
            );
            return;
        }
        match devices.record(
            contact,
            root_public_key,
            crate::contact_devices::Sibling {
                device_id: cert.device_id,
                ticket_text: device_ticket.to_owned(),
                added_at_millis: millis_now(),
            },
        ) {
            Ok(crate::contact_devices::Recorded::New) => {
                if let Err(e) = devices.save(&self.data_dir) {
                    tracing::warn!(error = %e, "couldn’t save the contact’s device");
                }
                drop(devices);
                let label = self.contact_label(contact);
                self.hub.notify(
                    NoticeLevel::Info,
                    format!("{label} added a new device. Messages you send them now go to both."),
                );
            }
            Ok(crate::contact_devices::Recorded::Known) => {}
            Err(why) => {
                drop(devices);
                tracing::warn!(error = %why, "couldn’t record a contact’s device");
            }
        }
    }

    fn contact_label(&self, conversation: siar_domain::ConversationId) -> String {
        self.hub
            .read(|s| {
                s.conversations
                    .ordered()
                    .iter()
                    .find(|c| c.id == conversation)
                    .map(|c| c.display_name.clone())
            })
            .unwrap_or_else(|| "A contact".to_owned())
    }

    /// Pair with the contacts a linked device was given. Returns how many were
    /// newly added. They are remembered in `imported_contacts.bin` so they
    /// survive a restart.
    fn import_contacts(&self, tickets: &[String]) -> usize {
        let mut added = 0;
        let mut keep = load_imported(&self.data_dir);
        for text in tickets {
            let Ok(ticket) = PeerTicket::decode(text) else {
                continue;
            };
            if !self.register_contact(ticket) {
                continue;
            }
            if !keep.contains(text) {
                keep.push(text.clone());
            }
            added += 1;
        }
        if let Err(e) = save_imported(&self.data_dir, &keep) {
            tracing::warn!(error = %e, "couldn’t remember the imported contacts");
        }
        added
    }

    /// Register a peer and show its conversation, without selecting it.
    /// `false` if it was already known.
    pub(crate) fn register_contact(&self, ticket: PeerTicket) -> bool {
        let known = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .conversation_of(&ticket.endpoint_addr.id)
            .is_some();
        if known {
            return false;
        }
        let title = ticket.endpoint_addr.id.fmt_short().to_string();
        let conversation = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .register(ticket);
        self.hub.update(|s| {
            s.ensure_conversation(conversation, title, siar_ui_state::ConversationKind::Direct);
            s.sendable.insert(conversation);
        });
        true
    }

    fn adopt_account(
        &self,
        account: AccountId,
        root_public_key: [u8; 32],
        certificate: Vec<u8>,
        approver: LinkedDevice,
    ) -> Result<(), String> {
        let dir: PathBuf = self.data_dir.clone();
        let joined = postcard::to_allocvec(&JoinedFile {
            account_id: account,
            root_public_key,
            certificate,
        })
        .map_err(|e| e.to_string())?;
        save_linked(&dir, &[approver]).map_err(|e| format!("couldn’t save the link: {e}"))?;
        write_private(&dir.join(JOINED_FILE), &joined, false)
            .map_err(|e| format!("couldn’t save the link: {e}"))?;
        std::fs::write(dir.join(ACCOUNT_FILE), account.as_uuid().to_string())
            .map_err(|e| format!("couldn’t save the account: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-link-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn this() -> DeviceSecurityView {
        DeviceSecurityView {
            id: DeviceId::new(),
            display_name: "me".into(),
            kind: siar_ui_state::DeviceKind::Desktop,
            status: DeviceTrustState::Trusted,
            added_at_millis: 1,
            last_active_millis: None,
            current_device: true,
            security_flags: vec![],
        }
    }

    #[test]
    fn the_root_key_is_created_once_stored_privately_and_reloaded() {
        let d = scratch("root");
        let a = root_key_for_offers(&d).unwrap();
        let b = root_key_for_offers(&d).unwrap();
        assert_eq!(a.root_public_key(), b.root_public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(d.join(ROOT_KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "key file must be owner-only");
        }
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_joined_device_never_invents_its_own_root() {
        let d = scratch("joined");
        std::fs::write(d.join(JOINED_FILE), b"x").unwrap();
        let e = root_key_for_offers(&d).err().unwrap();
        assert!(e.contains("joined"), "{e}");
        assert!(!d.join(ROOT_KEY_FILE).exists());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn linked_devices_survive_a_restart_and_show_after_this_device() {
        let d = scratch("linked");
        assert!(load_linked(&d).is_empty());
        let l = LinkedDevice {
            device_id: DeviceId::new(),
            name: "phone".into(),
            public_key: [4; 32],
            added_at_millis: 9,
            ticket: "t".into(),
            certificate: vec![1],
        };
        save_linked(&d, std::slice::from_ref(&l)).unwrap();
        assert_eq!(load_linked(&d), vec![l.clone()]);
        let list = device_list(this(), &load_linked(&d));
        assert_eq!(list.len(), 2);
        assert!(list[0].current_device && !list[1].current_device);
        assert_eq!(list[1].id, l.device_id);
        assert_eq!(
            list[1].security_flags,
            vec![DeviceSecurityFlag::UnreviewedSinceLinking]
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_damaged_linked_file_means_no_linked_devices_not_a_crash() {
        let d = scratch("damaged");
        std::fs::write(d.join(LINKED_FILE), b"\xff\xff garbage").unwrap();
        assert!(load_linked(&d).is_empty());
        std::fs::remove_dir_all(d).ok();
    }
}
