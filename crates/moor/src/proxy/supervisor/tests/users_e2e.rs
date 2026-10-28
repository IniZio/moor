//! Two users' Boxes on one store: each Proxy injects only its own user's secret, read from the
//! vault on every request, and a deleted secret fails the next request.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};
use ureq::Agent;

use crate::proxy::detached::{read_spec, write_spec};
use crate::proxy::nono::ca::ca_dir;
use crate::proxy::nono::capture::SecretsFn;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::proxy::supervisor::{serve, Deps, Sink, LEASE};
use crate::store::{Store, STORE_FILE};
use crate::testing::bridge::bridge;
use crate::testing::clock::FakeClock;
use crate::testing::cx::put_secret;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;
use crate::vault::Vault;

/// Each user's token; neither may appear anywhere but in the upstream request.
const ANN: &str = "ann-token-0f3c";
const BOB: &str = "bob-token-9a71";
/// Longest the test waits for a supervisor.
const DEADLINE: Duration = Duration::from_secs(20);

/// Raises the stop flag when dropped, so a failed assertion cannot leave `serve` running.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A Box named `name` for `user`, with a live machine from `driver` and a reserved proxy row.
fn box_for(store: &Store, driver: &FakeDriver, name: &str, user: UserId) -> (BoxId, BoxHandle) {
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let id = store
        .apply(Decision::insert(new).requester(user))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let got = driver
        .acquire(&AcquireReq {
            provider: provider.clone(),
            box_id: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: format!("n-{name}"),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(provider),
            ..Fields::default()
        });
    store.apply(admitted).unwrap();
    let ready = Decision::new(id)
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
    store.apply(ready).unwrap();
    drop(store.reserve(id, &FakeRng::default(), LEASE).unwrap());
    (id, got.handle)
}

/// The `claude` rule pointed at the fake upstream on `port`, allowing `GET /ok`.
fn claude_rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "claude".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "claude".to_owned(),
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

/// One state dir holding two users, Ann and Bob, each with a `claude` secret and a Box.
struct Two {
    dir: tempfile::TempDir,
    up: Upstream,
    store: Store,
    shared: Arc<Store>,
    driver: FakeDriver,
    proc: crate::testing::proc::ScriptedProc,
    rng: FakeRng,
    ann: (UserId, BoxId, BoxHandle),
    bob: (UserId, BoxId, BoxHandle),
}

fn two() -> Two {
    let dir = tempfile::tempdir().unwrap();
    let up = upstream::start(dir.path());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock.clone()).unwrap();
    let shared = Arc::new(Store::open(&dir.path().join(STORE_FILE), clock).unwrap());
    let key = dir.path().join("vault.key");
    let vault = Vault::load_or_create(&key).unwrap();
    let driver = FakeDriver::new().reverse_tunnels();
    let mut boxes = [("ann", ANN, 1), ("bob", BOB, 2)]
        .map(|(name, token, uid)| {
            let (user, _) = store.enroll(&Identity::local(uid), None).unwrap();
            put_secret(&store, &vault, user, "claude", token);
            let (id, handle) = box_for(&store, &driver, name, user);
            let spec = ProxySpec {
                box_id: id,
                box_handle: Some(handle.clone()),
                box_repo: None,
                rules: vec![claude_rule(up.port)],
                user,
                vault_key: key.clone(),
            };
            write_spec(dir.path(), &spec).unwrap();
            (user, id, handle)
        })
        .into_iter();
    let (ann, bob) = (boxes.next().unwrap(), boxes.next().unwrap());
    Two {
        dir,
        up,
        store,
        shared,
        driver,
        proc: crate::testing::proc::ScriptedProc::new(),
        rng: FakeRng::default(),
        ann,
        bob,
    }
}

impl Two {
    /// Deps whose secrets come from the store, as `moor proxy serve` wires them.
    fn deps<'a>(&'a self, sockets: &'a Path, stop: &Arc<AtomicBool>, err: Sink) -> Deps<'a> {
        let shared = Arc::clone(&self.shared);
        let secrets: SecretsFn = Arc::new(move |vault, user, name| {
            shared
                .user_secret(vault, user, name)
                .map_err(|e| e.to_string())
        });
        Deps {
            store: &self.store,
            driver: &self.driver,
            proc: &self.proc,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            rng: &self.rng,
            secrets,
            upstream_ca: Some(self.up.ca_path.clone()),
            upstream_dial: None,
            machine_service: None,
            guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, self.up.port))],
            state_dir: self.dir.path(),
            sockets,
            owner: rustix::process::getuid().as_raw(),
            boot_id: None,
            stop: Arc::clone(stop),
            stderr: err,
        }
    }

    /// A Box client through the Proxy of the Box behind `handle`, once its tunnel is up.
    fn client(&self, id: BoxId, handle: &BoxHandle) -> Agent {
        wait_for("the proxy row to be ready", || {
            self.store.proxy(id).unwrap().filter(|r| r.ready)
        });
        let socket = wait_for("the Box's tunnel", || {
            let tunnels = self.driver.tunnels();
            tunnels
                .into_iter()
                .find(|t| &t.handle == handle)
                .map(|t| t.socket)
        });
        let ca = crate::proxy::nono::ca::load_or_create(self.dir.path(), id).unwrap();
        upstream::box_client(bridge(&socket), ca.cert_pem())
    }

    /// The `Authorization` the upstream saw on its last request.
    fn last_auth(&self) -> Option<String> {
        self.up.seen().last().and_then(|s| s.authorization.clone())
    }

    /// Every file under the state dir that a secret must never reach: the specs, the
    /// Proxy logs and the store.
    fn files(&self) -> Vec<PathBuf> {
        let logs = std::fs::read_dir(self.dir.path().join("proxy")).unwrap();
        let mut out: Vec<PathBuf> = logs.map(|e| e.unwrap().path()).collect();
        out.retain(|p| p.is_file());
        out.extend(
            [self.ann.1, self.bob.1].map(|id| ca_dir(self.dir.path(), id).join("spec.json")),
        );
        out.extend(["", "-wal", "-shm"].map(|s| self.dir.path().join(format!("{STORE_FILE}{s}"))));
        out
    }
}

/// GETs `/ok` on the upstream; status or error text.
fn get(agent: &Agent, up: &Upstream) -> Result<u16, String> {
    agent
        .get(format!("https://localhost:{}/ok", up.port))
        .header("Authorization", "Bearer placeholder")
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}

/// Asserts the upstream's last request carried `token`.
fn assert_last(t: &Two, token: &str) {
    let want = format!("Bearer {token}");
    assert_eq!(t.last_auth().as_deref(), Some(want.as_str()));
}

/// Deletes Ann's secret; her next request fails before upstream and is recorded.
fn logout_ann(t: &Two, ann: &Agent) {
    let claude = SecretName::parse("claude").unwrap();
    assert!(t.store.delete_user_secret(t.ann.0, &claude).unwrap());
    let seen = t.up.seen().len();
    assert_ne!(get(ann, &t.up), Ok(200), "a deleted secret still worked");
    assert_eq!(
        t.up.seen().len(),
        seen,
        "the failed request reached upstream"
    );
    wait_for("the credential failure event", || {
        let events = t.store.events(t.ann.1, 100).unwrap();
        events
            .into_iter()
            .find(|e| e.kind == "proxy_credential_failed")
            .map(drop)
    });
}

/// Asserts neither token nor the vault key is in `said`, the specs, the logs, the events or
/// the store.
fn assert_nowhere(t: &Two, mut said: Vec<u8>) {
    for path in t.files() {
        said.extend(std::fs::read(&path).unwrap_or_default());
    }
    for event in [t.ann.1, t.bob.1].map(|id| t.store.events(id, 100).unwrap()) {
        said.extend(format!("{event:?}").into_bytes());
    }
    let key = std::fs::read_to_string(t.dir.path().join("vault.key")).unwrap();
    for secret in [ANN, BOB, key.trim()] {
        let found = said.windows(secret.len()).any(|w| w == secret.as_bytes());
        assert!(!found, "a secret reached a spec, log, event or the store");
    }
}

#[test]
fn each_users_box_injects_only_that_users_secret_and_a_deleted_one_fails_next() {
    let t = two();
    let sockets = t.dir.path().join("s");
    let stop = Arc::new(AtomicBool::new(false));
    let err = Arc::new(Mutex::new(Vec::new()));
    let deps = t.deps(&sockets, &stop, Arc::clone(&err) as Sink);
    std::thread::scope(|s| {
        let servers = [t.ann.1, t.bob.1].map(|id| {
            let deps = &deps;
            s.spawn(move || serve(deps, id))
        });
        let _guard = StopOnDrop(Arc::clone(&stop));
        let (ann, bob) = (t.client(t.ann.1, &t.ann.2), t.client(t.bob.1, &t.bob.2));
        assert_eq!(get(&ann, &t.up), Ok(200));
        assert_last(&t, ANN);
        assert_eq!(get(&bob, &t.up), Ok(200));
        assert_last(&t, BOB);
        logout_ann(&t, &ann);
        assert_eq!(get(&bob, &t.up), Ok(200), "Bob's Box kept working");
        assert_last(&t, BOB);
        stop.store(true, Ordering::Release);
        for server in servers {
            assert_eq!(server.join().unwrap(), 0);
        }
    });
    let failures = t.store.events(t.ann.1, 100).unwrap();
    let failures = failures
        .iter()
        .filter(|e| e.kind == "proxy_credential_failed");
    assert_eq!(
        failures.count(),
        1,
        "one failed request records one credential failure"
    );
    let said = err.lock().unwrap().clone();
    assert_nowhere(&t, said);
}

#[test]
fn a_proxy_with_a_missing_vault_key_refuses_to_start_and_creates_none() {
    let t = two();
    let missing = t.dir.path().join("no-such.key");
    let spec = ProxySpec {
        vault_key: missing.clone(),
        ..read_spec(t.dir.path(), t.ann.1).unwrap()
    };
    write_spec(t.dir.path(), &spec).unwrap();
    let sockets = t.dir.path().join("s");
    let stop = Arc::new(AtomicBool::new(false));
    let err = Arc::new(Mutex::new(Vec::new()));
    let deps = t.deps(&sockets, &stop, Arc::clone(&err) as Sink);
    let watchdog = Arc::clone(&stop);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        watchdog.store(true, Ordering::Release);
    });
    assert_ne!(
        serve(&deps, t.ann.1),
        0,
        "the Proxy started without its vault key"
    );
    assert!(!missing.exists(), "the Proxy created a vault key");
}
