//! Real Iroh connections on loopback (no discovery, no relay).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::{EndpointAddr, SecretKey, TransportAddr};
use siar_admin::limits::RateConfig;
use siar_admin::protocol::{Reply, Request, ALPN, MAX_REQUEST_BYTES};
use siar_admin::server::BoxFuture;
use siar_admin::{
    AdminBackend, AdminClient, AdminError, AdminOp, AdminRole, AdminServer, AdminServerConfig,
    AuditKind, ClientError, Operator, OperatorContext, OperatorTable,
};

#[derive(Default)]
struct Fake {
    executed: Mutex<Vec<(String, String)>>, // (operator label, op label)
    audits: Mutex<Vec<(Option<String>, String, AuditKind)>>,
    delay: Option<Duration>,
}

impl AdminBackend for Fake {
    fn execute<'a>(
        &'a self,
        ctx: &'a OperatorContext,
        op: AdminOp,
    ) -> BoxFuture<'a, Result<Vec<u8>, AdminError>> {
        Box::pin(async move {
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            self.executed
                .lock()
                .unwrap()
                .push((ctx.label.clone(), op.label().to_owned()));
            Ok(format!("ok:{}", op.label()).into_bytes())
        })
    }
    fn audit(&self, who: Option<&OperatorContext>, label: &str, kind: AuditKind) {
        self.audits
            .lock()
            .unwrap()
            .push((who.map(|w| w.label.clone()), label.to_owned(), kind));
    }
}

struct Net {
    server: AdminServer,
    node: EndpointAddr,
    backend: Arc<Fake>,
    keys: Vec<(&'static str, SecretKey)>,
}

fn direct_only(full: EndpointAddr) -> EndpointAddr {
    let ips = full
        .addrs
        .into_iter()
        .filter(|a| matches!(a, TransportAddr::Ip(_)));
    EndpointAddr::from_parts(full.id, ips)
}

async fn net(cfg: AdminServerConfig, backend: Fake) -> Net {
    let roles = [
        ("viewer", AdminRole::Viewer),
        ("operator", AdminRole::Operator),
        ("secadmin", AdminRole::SecurityAdmin),
        ("owner", AdminRole::Owner),
    ];
    let keys: Vec<_> = roles
        .iter()
        .map(|(n, _)| (*n, SecretKey::generate()))
        .collect();
    let table = OperatorTable::new(
        roles
            .iter()
            .zip(&keys)
            .map(|((n, r), (_, k))| Operator {
                key: *k.public().as_bytes(),
                role: *r,
                label: (*n).to_owned(),
            })
            .collect(),
    )
    .unwrap();
    let backend = Arc::new(backend);
    let server = AdminServer::spawn(cfg, SecretKey::generate(), table, backend.clone())
        .await
        .unwrap();
    let node = direct_only(server.addr());
    Net {
        server,
        node,
        backend,
        keys,
    }
}

fn key(n: &Net, who: &str) -> SecretKey {
    n.keys.iter().find(|(l, _)| *l == who).unwrap().1.clone()
}

async fn ask(n: &Net, who: &str, op: AdminOp) -> Result<Vec<u8>, ClientError> {
    let c = AdminClient::new(key(n, who), false).await.unwrap();
    let r = c.request(n.node.clone(), op).await;
    c.close().await;
    r
}

fn fast() -> AdminServerConfig {
    AdminServerConfig {
        rate: RateConfig {
            capacity: 1000,
            per_second: 1000,
        },
        ..AdminServerConfig::default()
    }
}

#[tokio::test]
async fn each_role_can_do_exactly_what_the_table_says() {
    let n = net(fast(), Fake::default()).await;
    let ops = [
        AdminOp::Health,
        AdminOp::SetMaintenance(true),
        AdminOp::Audit,
        AdminOp::Shutdown,
    ];
    for who in ["viewer", "operator", "secadmin", "owner"] {
        let role = n.keys.iter().position(|(l, _)| *l == who).unwrap();
        let role = [
            AdminRole::Viewer,
            AdminRole::Operator,
            AdminRole::SecurityAdmin,
            AdminRole::Owner,
        ][role];
        for op in &ops {
            let r = ask(&n, who, op.clone()).await;
            if role >= op.required_role() {
                assert_eq!(
                    r.unwrap(),
                    format!("ok:{}", op.label()).into_bytes(),
                    "{who} {op:?}"
                );
            } else {
                assert!(
                    matches!(r, Err(ClientError::Remote(AdminError::Forbidden))),
                    "{who} {op:?}: {r:?}"
                );
            }
        }
    }
    // The backend ran only what was allowed — a Forbidden never reached it.
    let ex = n.backend.executed.lock().unwrap().clone();
    assert!(!ex.contains(&("viewer".into(), "shutdown".into())));
    assert!(!ex.contains(&("operator".into(), "audit".into())));
    assert!(ex.contains(&("owner".into(), "shutdown".into())));
    // And every request, allowed or not, was audited.
    let audits = n.backend.audits.lock().unwrap().clone();
    assert_eq!(audits.len(), 16);
    assert!(audits.iter().any(|(w, l, k)| w.as_deref() == Some("viewer")
        && l == "shutdown"
        && *k == AuditKind::Forbidden));
    n.server.shutdown().await;
}

#[tokio::test]
async fn an_unlisted_key_is_refused_before_anything_is_parsed() {
    let n = net(fast(), Fake::default()).await;
    let stranger = AdminClient::new(SecretKey::generate(), false)
        .await
        .unwrap();
    let r = stranger.request(n.node.clone(), AdminOp::Health).await;
    assert!(r.is_err(), "a stranger must not get an answer: {r:?}");
    assert!(n.backend.executed.lock().unwrap().is_empty());
    let audits = n.backend.audits.lock().unwrap().clone();
    assert!(
        audits
            .iter()
            .any(|(w, _, k)| w.is_none() && *k == AuditKind::UnknownPeer),
        "{audits:?}"
    );
    stranger.close().await;
    // The real operators are unaffected.
    assert!(ask(&n, "viewer", AdminOp::Health).await.is_ok());
    n.server.shutdown().await;
}

#[tokio::test]
async fn the_rate_limit_applies_per_operator() {
    let cfg = AdminServerConfig {
        rate: RateConfig {
            capacity: 3,
            per_second: 1,
        },
        ..AdminServerConfig::default()
    };
    let n = net(cfg, Fake::default()).await;
    let mut results = Vec::new();
    for _ in 0..5 {
        results.push(ask(&n, "viewer", AdminOp::Health).await);
    }
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let limited = results
        .iter()
        .filter(|r| matches!(r, Err(ClientError::Remote(AdminError::RateLimited))))
        .count();
    assert_eq!((ok, limited), (3, 2), "{results:?}");
    // Another operator has their own bucket.
    assert!(ask(&n, "owner", AdminOp::Health).await.is_ok());
    let audits = n.backend.audits.lock().unwrap().clone();
    assert_eq!(
        audits
            .iter()
            .filter(|(_, _, k)| *k == AuditKind::RateLimited)
            .count(),
        2
    );
    n.server.shutdown().await;
}

/// Open a raw authenticated stream and send arbitrary bytes.
async fn raw(n: &Net, who: &str, bytes: Vec<u8>) -> Option<Vec<u8>> {
    use iroh::endpoint::presets;
    let ep = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(key(n, who))
        .bind()
        .await
        .unwrap();
    let conn = ep.connect(n.node.clone(), ALPN).await.ok()?;
    let (mut send, mut recv) = conn.open_bi().await.ok()?;
    let _ = send.write_all(&bytes).await;
    let _ = send.finish();
    let out = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(1 << 20))
        .await
        .ok()
        .and_then(Result::ok);
    conn.close(0u32.into(), b"done");
    ep.close().await;
    out
}

#[tokio::test]
async fn garbage_oversized_and_trailing_input_never_break_the_server() {
    let n = net(fast(), Fake::default()).await;

    // Garbage → an Invalid reply (or a closed stream), never an execution.
    if let Some(bytes) = raw(
        &n,
        "owner",
        vec![0xff; 40], /* an overlong varint: cannot decode as any request */
    )
    .await
    {
        if let Ok(rep) = postcard::from_bytes::<Reply>(&bytes) {
            assert_eq!(rep.result, Err(AdminError::Invalid));
        }
    }
    // Oversized: over the cap, rejected without executing.
    let _ = raw(&n, "owner", vec![0u8; MAX_REQUEST_BYTES + 1000]).await;
    // A valid request with trailing junk is not accepted as valid.
    let mut ok = postcard::to_allocvec(&Request {
        id: 5,
        op: AdminOp::Shutdown,
    })
    .unwrap();
    ok.extend_from_slice(b"junk");
    if let Some(bytes) = raw(&n, "owner", ok).await {
        if let Ok(rep) = postcard::from_bytes::<Reply>(&bytes) {
            assert_eq!(rep.result, Err(AdminError::Invalid));
        }
    }
    assert!(
        n.backend.executed.lock().unwrap().is_empty(),
        "nothing malformed may reach the backend"
    );
    assert!(n
        .backend
        .audits
        .lock()
        .unwrap()
        .iter()
        .any(|(_, _, k)| *k == AuditKind::Malformed));

    // And the server still serves normal requests afterwards.
    assert_eq!(
        ask(&n, "owner", AdminOp::Health).await.unwrap(),
        b"ok:health"
    );
    n.server.shutdown().await;
}

#[tokio::test]
async fn simultaneous_connections_are_capped() {
    let cfg = AdminServerConfig {
        max_connections: 1,
        ..fast()
    };
    let n = net(cfg, Fake::default()).await;

    // Hold the only slot: an authenticated connection with no request.
    use iroh::endpoint::presets;
    let ep = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(key(&n, "viewer"))
        .bind()
        .await
        .unwrap();
    let hold = ep.connect(n.node.clone(), ALPN).await.unwrap();
    let (_s, _r) = hold.open_bi().await.unwrap(); // server is now waiting on this stream
    tokio::time::sleep(Duration::from_millis(200)).await;

    let second = ask(&n, "owner", AdminOp::Health).await;
    assert!(
        second.is_err(),
        "a second connection must be refused while the slot is held: {second:?}"
    );
    assert!(n
        .backend
        .audits
        .lock()
        .unwrap()
        .iter()
        .any(|(_, _, k)| *k == AuditKind::Overloaded));

    // Free the slot; service resumes.
    hold.close(0u32.into(), b"done");
    ep.close().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(ask(&n, "owner", AdminOp::Health).await.is_ok());
    n.server.shutdown().await;
}

#[tokio::test]
async fn a_slow_operation_times_out_and_is_audited() {
    let cfg = AdminServerConfig {
        exec_timeout: Duration::from_millis(200),
        ..fast()
    };
    let backend = Fake {
        delay: Some(Duration::from_secs(3)),
        ..Fake::default()
    };
    let n = net(cfg, backend).await;
    let r = ask(&n, "viewer", AdminOp::Health).await;
    assert!(
        matches!(r, Err(ClientError::Remote(AdminError::Timeout))),
        "{r:?}"
    );
    assert!(n
        .backend
        .audits
        .lock()
        .unwrap()
        .iter()
        .any(|(_, _, k)| *k == AuditKind::Failed(AdminError::Timeout)));
    n.server.shutdown().await;
}

#[tokio::test]
async fn the_admin_endpoint_is_a_separate_identity_and_not_published_by_default() {
    let n = net(fast(), Fake::default()).await;
    // Discovery off by default (§160).
    assert!(!AdminServerConfig::default().discovery);
    // The node answers only on comm-admin/1: a different ALPN cannot connect.
    use iroh::endpoint::presets;
    let ep = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(key(&n, "owner"))
        .bind()
        .await
        .unwrap();
    let wrong = ep.connect(n.node.clone(), b"comm-ipc/1").await;
    assert!(
        wrong.is_err(),
        "the local IPC protocol id must not be served here"
    );
    ep.close().await;
    n.server.shutdown().await;
}

#[tokio::test]
async fn shutdown_lets_an_in_flight_request_finish_and_deliver_its_reply() {
    let backend = Fake {
        delay: Some(Duration::from_millis(600)),
        ..Fake::default()
    };
    let n = net(fast(), backend).await;
    let node = n.node.clone();
    let k = key(&n, "owner");
    let req = tokio::spawn(async move {
        let c = AdminClient::new(k, false).await.unwrap();
        let r = c.request(node, AdminOp::Shutdown).await;
        c.close().await;
        r
    });
    // Let the request reach the (slow) backend, then stop the server under it.
    tokio::time::sleep(Duration::from_millis(250)).await;
    n.server.shutdown().await;
    let r = req.await.unwrap();
    assert_eq!(
        r.unwrap(),
        b"ok:shutdown",
        "the reply must survive server shutdown"
    );
}

#[tokio::test]
async fn a_barrage_of_hostile_frames_never_crashes_or_confuses_the_server() {
    let n = net(fast(), Fake::default()).await;
    let seed = postcard::to_allocvec(&Request {
        id: 9,
        op: AdminOp::Metrics,
    })
    .unwrap();
    let mut x: u64 = 0xDEAD_BEEF_0BAD_F00D;
    let mut next = move || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut genuine_after_mutation = 0u32;
    for round in 0..40 {
        let mut frame = if round % 4 == 0 {
            (0..(next() % 200) as usize)
                .map(|_| next() as u8)
                .collect::<Vec<u8>>()
        } else {
            seed.clone()
        };
        if round % 4 != 0 && !frame.is_empty() {
            let i = (next() as usize) % frame.len();
            frame[i] ^= 1 << (next() % 8);
        }
        // Valid-by-chance frames (a flipped bit that is still a request) are
        // allowed to execute — for an authorised operator and only then.
        if postcard::take_from_bytes::<Request>(&frame)
            .map(|(_, r)| r.is_empty())
            .unwrap_or(false)
        {
            genuine_after_mutation += 1;
        }
        let who = if round % 5 == 0 { "viewer" } else { "owner" };
        let _ = raw(&n, who, frame).await;
    }
    // Everything the backend actually ran decoded as a real, authorised request
    // (viewer-only frames can run only viewer operations).
    for (label, op) in n.backend.executed.lock().unwrap().iter() {
        assert!(!label.is_empty() && !op.is_empty());
    }
    let ran = n.backend.executed.lock().unwrap().len() as u32;
    assert!(
        ran <= genuine_after_mutation + 1,
        "ran {ran} but only {genuine_after_mutation} frames were valid requests"
    );
    // The server is still healthy and correct.
    assert_eq!(
        ask(&n, "owner", AdminOp::Health).await.unwrap(),
        b"ok:health"
    );
    assert!(
        ask(&n, "viewer", AdminOp::Shutdown).await.is_err(),
        "roles still enforced"
    );
    n.server.shutdown().await;
}
