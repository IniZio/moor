//! `serve` end to end on loopback: a client goes through the nono proxy to a fake HTTPS upstream.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId, Secret};
use ureq::Agent;

use crate::proxy::detached::write_spec;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::proxy::supervisor::{serve, Deps, Sink, LEASE};
use crate::store::Store;
use crate::testing::bridge::bridge;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;

/// What the client sends before the proxy swaps it out.
const PLACEHOLDER: &str = "Bearer placeholder-not-real";
/// The credential the fake credential source returns.
const SECRET: &str = "test-token-1";
/// Longest the test waits for the supervisor.
const DEADLINE: Duration = Duration::from_secs(20);

/// Fake clock whose sleeps also pause for a short real time, so the supervisor ticks at a sane rate.
struct PacedClock(FakeClock);

impl Clock for PacedClock {
    fn now(&self) -> Ts {
        self.0.now()
    }

    fn instant(&self) -> Instant {
        self.0.instant()
    }

    fn sleep(&self, d: Duration) {
        self.0.sleep(d);
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Raises the stop flag when dropped, so a failed assertion cannot leave `serve` running.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Insert a Box, give it a live machine from `driver`, and reserve its proxy row.
fn box_with_machine(store: &Store, driver: &FakeDriver) -> (BoxId, BoxHandle) {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let provider = ProviderId::new("p");
    let got = driver
        .acquire(&AcquireReq {
            provider: provider.clone(),
            box_id: id,
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "abc123".into(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(provider),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    drop(store.reserve(id, &FakeRng::default(), LEASE).unwrap());
    (id, got.handle)
}

/// One header-injecting rule for `localhost:<port>` that allows only `GET /ok`.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "tool".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "tool-token".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// GETs `url` with the placeholder credential; status or error text.
fn get(agent: &Agent, url: &str) -> Result<u16, String> {
    agent
        .get(url)
        .header("Authorization", PLACEHOLDER)
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}

/// Polls until `done` yields a value or `DEADLINE` of real time has passed.
fn wait_for<T>(what: &str, mut done: impl FnMut() -> Option<T>) -> T {
    let step = Duration::from_millis(25);
    let tries = DEADLINE.as_millis() / step.as_millis();
    (0..tries)
        .find_map(|_| {
            let found = done();
            if found.is_none() {
                std::thread::sleep(step);
            }
            found
        })
        .unwrap_or_else(|| unreachable!("timed out waiting for {what}"))
}

/// Event kinds recorded for `box_id`.
fn kinds(store: &Store, box_id: BoxId) -> Vec<String> {
    let events = store.events(box_id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

/// An allowed request reaches the upstream with the real credential in place of the placeholder.
fn assert_injected(client: &Agent, up: &Upstream) {
    assert_eq!(
        get(client, &format!("https://localhost:{}/ok", up.port)),
        Ok(200)
    );
    let seen = up.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "GET /ok HTTP/1.1");
    assert_eq!(only.authorization.as_deref(), Some("Bearer test-token-1"));
}

/// Requests outside the rules never reach the upstream.
fn assert_refused(client: &Agent, up: &Upstream) {
    let denied = get(client, &format!("https://localhost:{}/denied", up.port));
    assert!(
        denied.is_err() || matches!(denied, Ok(403 | 502)),
        "got {denied:?}"
    );
    assert_eq!(up.seen().len(), 1);
    let other = get(client, "https://example.invalid/anything");
    assert!(other.is_err() || other == Ok(403), "got {other:?}");
}

/// The refusal is recorded as an event and the allowed request is logged.
fn assert_audited(store: &Store, id: BoxId, dir: &Path) {
    wait_for("a refused-egress event", || {
        kinds(store, id)
            .iter()
            .any(|k| k == "egress_refused")
            .then_some(())
    });
    let log = dir.join("proxy").join(format!("{}.log", id.get()));
    wait_for("the request log line", || {
        std::fs::read_to_string(&log).ok().filter(|text| {
            text.lines()
                .any(|l| l.contains(" GET ") && l.ends_with(" 200"))
        })
    });
}

/// Waits for the proxy row to be ready, then returns the socket the tunnel was opened to.
fn tunnel_socket(store: &Store, driver: &FakeDriver, id: BoxId) -> PathBuf {
    wait_for("the proxy row to be ready", || {
        store.proxy(id).unwrap().filter(|r| r.ready)
    });
    driver.tunnels().first().unwrap().socket.clone()
}

#[test]
fn serve_injects_the_credential_audits_requests_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let up: Upstream = upstream::start(dir.path());
    let clock = Arc::new(PacedClock(FakeClock::new(Ts::from_millis(1_000))));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let driver = FakeDriver::new().reverse_tunnels();
    let (id, handle) = box_with_machine(&store, &driver);
    let spec = ProxySpec {
        box_id: id,
        box_handle: Some(handle),
        box_repo: None,
        rules: vec![rule(up.port)],
        user: moor_core::user::UserId::new(1),
        vault_key: crate::testing::cx::test_vault_key(dir.path()),
    };
    write_spec(dir.path(), &spec).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let err: Sink = Arc::new(Mutex::new(Vec::new()));
    let rng = FakeRng::default();
    let deps = Deps {
        store: &store,
        driver: &driver,
        clock,
        rng: &rng,
        secrets: Arc::new(|_, _, name| {
            assert_eq!(name.as_str(), "tool-token");
            Ok(Some(Secret::new(SECRET.as_bytes().to_vec())))
        }),
        upstream_ca: Some(up.ca_path.clone()),
        upstream_dial: None,
        guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, up.port))],
        state_dir: dir.path(),
        sockets: &dir.path().join("s"),
        owner: rustix::process::getuid().as_raw(),
        boot_id: None,
        stop: Arc::clone(&stop),
        stderr: err,
    };

    let code = std::thread::scope(|s| {
        let server = s.spawn(|| serve(&deps, id));
        let _guard = StopOnDrop(Arc::clone(&stop));
        let socket = tunnel_socket(&store, &driver, id);
        let port = bridge(&socket);
        let ca = crate::proxy::nono::ca::load_or_create(dir.path(), id).unwrap();
        let client = upstream::box_client(port, ca.cert_pem());
        assert_injected(&client, &up);
        assert_refused(&client, &up);
        assert_audited(&store, id, dir.path());
        assert_eq!(up.seen().len(), 1);

        stop.store(true, Ordering::Release);
        let code = server.join().unwrap();
        assert!(UnixStream::connect(&socket).is_err());
        code
    });
    assert_eq!(code, 0);
    assert!(store.proxy(id).unwrap().is_some());
}
