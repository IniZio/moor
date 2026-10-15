//! `serve` end to end on loopback: a client goes through the nono proxy to a fake HTTPS upstream.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, ProviderId, Secret, TaskId, TaskName};
use ureq::Agent;

use crate::proxy::detached::write_spec;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::proxy::supervisor::{serve, Deps, Sink, LEASE};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::rng::FakeRng;

/// The Task Nonce the client uses as proxy password.
const NONCE: &str = "nonce_e2e-0123456789";
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

/// Insert a task, give it a live box from `driver`, and reserve its proxy row.
fn task_with_box(store: &Store, driver: &FakeDriver) -> (TaskId, BoxHandle) {
    let nt = NewTask {
        name: TaskName::parse("demo").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    let provider = ProviderId::new("p");
    let got = driver
        .acquire(&AcquireReq {
            provider: provider.clone(),
            task: id,
            name: TaskName::parse("demo").unwrap(),
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(provider),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            worktree: Field::Set("/wt".into()),
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    drop(
        store
            .reserve(id, &nonce, &FakeRng::default(), LEASE)
            .unwrap(),
    );
    (id, got.handle)
}

/// One header-injecting rule for `localhost:<port>` that allows only `GET /ok`.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "tool".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "TOOL_TOKEN".to_owned(),
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

/// Event kinds recorded for `task`.
fn kinds(store: &Store, task: TaskId) -> Vec<String> {
    let events = store.events(task, 100).unwrap();
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
fn assert_audited(store: &Store, id: TaskId, dir: &Path) {
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

#[test]
fn serve_injects_the_credential_audits_requests_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let up: Upstream = upstream::start(dir.path());
    let clock = Arc::new(PacedClock(FakeClock::new(Ts::from_millis(1_000))));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let driver = FakeDriver::new().reverse_tunnels();
    let (id, handle) = task_with_box(&store, &driver);
    let spec = ProxySpec {
        task: id,
        box_handle: Some(handle),
        host_cwd: "/wt".into(),
        task_repo: None,
        rules: vec![rule(up.port)],
        allow: Vec::new(),
    };
    write_spec(dir.path(), &spec).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let err: Sink = Arc::new(Mutex::new(Vec::new()));
    let rng = FakeRng::default();
    let deps = Deps {
        store: &store,
        driver: &driver,
        forge: Arc::new(FakeForge::new()),
        clock,
        rng: &rng,
        creds: Arc::new(|name: &str| {
            assert_eq!(name, "TOOL_TOKEN");
            Ok(Secret::new(SECRET.as_bytes().to_vec()))
        }),
        upstream_ca: Some(up.ca_path.clone()),
        upstream_dial: None,
        guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, up.port))],
        state_dir: dir.path(),
        boot_id: None,
        stop: Arc::clone(&stop),
        stderr: err,
    };

    let code = std::thread::scope(|s| {
        let server = s.spawn(|| serve(&deps, id));
        let _guard = StopOnDrop(Arc::clone(&stop));
        let port = wait_for("the proxy row to be ready", || {
            let row = store.proxy(id).unwrap()?;
            row.ready.then_some(row.host_port).flatten()
        });
        let ca = crate::proxy::nono::ca::load_or_create(dir.path(), id).unwrap();
        let client = upstream::client(port, NONCE, ca.cert_pem());
        assert_injected(&client, &up);
        assert_refused(&client, &up);
        assert_audited(&store, id, dir.path());
        let anon = upstream::client(port, "wrong-nonce", ca.cert_pem());
        let bad = get(&anon, &format!("https://localhost:{}/ok", up.port));
        assert!(bad.is_err() || bad == Ok(407), "got {bad:?}");
        assert_eq!(up.seen().len(), 1);

        stop.store(true, Ordering::Release);
        let code = server.join().unwrap();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err());
        code
    });
    assert_eq!(code, 0);
    assert!(store.proxy(id).unwrap().is_none());
}
