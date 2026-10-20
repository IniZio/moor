//! Tests for the per-Box proxy supervisor, run against fakes with no real sleeps.

use std::net::{Ipv4Addr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::{box_state::BoxLifecycleInput, session_state::SessionInput};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::secret::Secret;
use moor_core::{BoxId, BoxName, ProviderId};

mod evict;
mod front_e2e;
mod graphql_e2e;
mod hairpin_e2e;
mod nono_e2e;

use super::{serve, start, Deps, Sink, Stop, LEASE, TICK};
use crate::proxy::detached::{read_spec, write_spec};
use crate::proxy::nono::ca::ca_dir;
use crate::proxy::nono::capture::CredsFn;
use crate::store::{Lease, ProxyRow, Store};
use crate::testing::clock::FakeClock;
use crate::testing::driver::{FakeDriver, TunnelReply};
use crate::testing::forge::FakeForge;
use crate::testing::rng::FakeRng;

const NONCE: &[u8] = b"NONCE-abc123";
const BOOT: &str = "boot-1";
pub(super) const NONCE_TEXT: &str = "NONCE-abc123";
const WORKTREE: &str = "/wt";
pub(super) const FIRST_TUNNEL_PID: u32 = 50_000;

/// Everything a supervisor run needs, owned in one place.
pub(super) struct World {
    /// Keeps the state directory alive.
    pub(super) dir: tempfile::TempDir,
    /// Shared with the store and the supervisor.
    pub(super) clock: Arc<FakeClock>,
    /// Box and proxy storage.
    pub(super) store: Store,
    /// Fake box driver with reverse tunnels enabled.
    pub(super) driver: FakeDriver,
    /// Random source for port draws.
    pub(super) rng: FakeRng,
    /// Stop flag handed to the supervisor.
    pub(super) stop: Arc<AtomicBool>,
    /// Bytes the supervisor wrote to stderr.
    pub(super) err: Arc<Mutex<Vec<u8>>>,
    /// The Box under test.
    pub(super) id: BoxId,
}

impl World {
    /// A working Box with a live machine, a worktree and a reserved proxy row.
    pub(super) fn new(replies: impl IntoIterator<Item = TunnelReply>) -> Self {
        Self::with_spec(replies, |_| {})
    }

    /// Like [`World::new`], with `edit` applied to the spec written for the Box.
    pub(super) fn with_spec(
        replies: impl IntoIterator<Item = TunnelReply>,
        edit: impl FnOnce(&mut ProxySpec),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
        let driver = FakeDriver::new().reverse_tunnels().tunnel_replies(replies);
        let id = working_box(&store, &driver);
        let nonce = Secret::new(NONCE.to_vec());
        drop(
            store
                .reserve(id, &nonce, &FakeRng::default(), LEASE)
                .unwrap(),
        );
        let mut spec = ProxySpec {
            box_id: id,
            box_handle: None,
            host_cwd: PathBuf::from(WORKTREE),
            box_repo: Some(ForgeRepo::new("o", "n")),
            rules: Vec::new(),
        };
        edit(&mut spec);
        write_spec(dir.path(), &spec).unwrap();
        Self {
            dir,
            clock,
            store,
            driver,
            rng: FakeRng::default(),
            stop: Arc::new(AtomicBool::new(false)),
            err: Arc::new(Mutex::new(Vec::new())),
            id,
        }
    }

    /// Dependencies wired to this world's fakes; the credential lookup always fails.
    pub(super) fn deps(&self) -> Deps<'_> {
        self.deps_with(self.clock.clone())
    }

    /// Dependencies using `clock` instead of the plain fake clock.
    pub(super) fn deps_with(&self, clock: Arc<dyn Clock>) -> Deps<'_> {
        self.deps_full(clock, no_creds(), None)
    }

    /// Dependencies with the given credential lookup and extra upstream root PEM.
    pub(super) fn deps_full(
        &self,
        clock: Arc<dyn Clock>,
        creds: CredsFn,
        upstream_ca: Option<PathBuf>,
    ) -> Deps<'_> {
        let forge = FakeForge::new().with_token(ForgeRepo::new("o", "n"), b"gh-token");
        let err: Sink = self.err.clone();
        Deps {
            store: &self.store,
            driver: &self.driver,
            forge: Arc::new(forge),
            clock,
            rng: &self.rng,
            creds,
            upstream_dial: None,
            guard_exempt: Vec::new(),
            upstream_ca,
            state_dir: self.dir.path(),
            boot_id: Some(BOOT.to_owned()),
            stop: self.stop.clone(),
            stderr: err,
        }
    }

    /// The proxy row, which must exist.
    pub(super) fn row(&self) -> ProxyRow {
        self.store.proxy(self.id).unwrap().unwrap()
    }

    /// Everything written to stderr so far.
    pub(super) fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.err.lock().unwrap()).into_owned()
    }

    /// Mark the Box closed and its machine released.
    pub(super) fn close_box(&self) {
        let d = Decision::new(self.id)
            .lifecycle(BoxLifecycleInput::Close)
            .session(SessionInput::End)
            .box_input(BoxInput::Released);
        self.store.apply(d).unwrap();
    }

    /// The proxy log file contents, empty when none was written.
    pub(super) fn log(&self) -> String {
        let path: PathBuf = self
            .dir
            .path()
            .join("proxy")
            .join(format!("{}.log", self.id.get()));
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Proxy events recorded for the Box.
    pub(super) fn started_events(&self) -> Vec<serde_json::Value> {
        let events = self.store.events(self.id, 50).unwrap();
        let started = events.into_iter().filter(|e| e.kind == "proxy_started");
        started.map(|e| e.payload).collect()
    }
}

/// A credential lookup that has no credentials.
pub(super) fn no_creds() -> CredsFn {
    Arc::new(|_: &str| Err("no credential in tests".to_owned()))
}

/// Insert a Box, give it a live machine from `driver` and a worktree.
fn working_box(store: &Store, driver: &FakeDriver) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: id,
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            worktree: Field::Set(WORKTREE.into()),
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    id
}

/// Clock that raises the stop flag on its `n`th sleep.
struct StopOnSleep {
    /// Underlying fake clock.
    inner: Arc<FakeClock>,
    /// Flag to raise.
    stop: Arc<AtomicBool>,
    /// Sleeps left before the flag goes up.
    left: AtomicUsize,
}

impl Clock for StopOnSleep {
    fn now(&self) -> Ts {
        self.inner.now()
    }

    fn instant(&self) -> Instant {
        self.inner.instant()
    }

    fn sleep(&self, d: Duration) {
        self.inner.sleep(d);
        if self.left.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn second_supervisor_exits_zero_and_leaves_the_row_alone() {
    let w = World::new([]);
    let held = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(held, Lease::Claimed));
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(0));

    let row = w.row();
    assert!(!row.ready);
    assert_eq!(row.lease_owner.as_deref(), Some("other-owner"));
    assert!(w.driver.tunnels().is_empty());
    assert!(w.started_events().is_empty());
}

#[test]
fn tunnel_failure_releases_the_lease_and_keeps_the_row_unready() {
    let w = World::new([TunnelReply::EarlyExit {
        code: 7,
        stderr: "denied".into(),
    }]);
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(1));

    let row = w.row();
    assert!(!row.ready);
    assert_eq!(row.lease_owner, None);
    assert!(w.stderr().to_lowercase().contains("tunnel"));
    assert!(w.started_events().is_empty());
}

#[test]
fn successful_start_marks_ready_and_records_ports_without_the_nonce() {
    let w = World::new([]);
    let deps = w.deps();

    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    let row = w.row();
    assert!(row.ready);
    assert_eq!(row.boot_id.as_deref(), Some(BOOT));
    assert_eq!(row.host_port, Some(run.host_port()));
    assert_eq!(row.lease_owner.as_deref(), Some(run.owner()));
    assert_started_event(&w, &row, run.host_port());
    let calls = w.driver.tunnels();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().unwrap().box_port, row.box_port);
    assert_eq!(calls.first().unwrap().host_port, run.host_port());
}

/// Checks the single `proxy_started` event: ports, pid and handle, and no nonce.
fn assert_started_event(w: &World, row: &ProxyRow, host_port: u16) {
    let events = w.started_events();
    assert_eq!(events.len(), 1);
    let p = events.first().unwrap();
    assert_eq!(p["box_port"], row.box_port);
    assert_eq!(p["host_port"], host_port);
    assert_eq!(p["pid"], row.pid.unwrap());
    assert_eq!(p["handle"], row.handle.clone().unwrap());
    assert!(!p.to_string().contains(NONCE_TEXT));
}

#[test]
fn closed_box_stops_the_supervisor_and_removes_the_row() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.close_box();
    let stop = run.tick();

    assert!(matches!(stop, Some(Stop::Closed)));
    assert_eq!(run.finish(Stop::Closed), 0);
    assert!(w.store.proxy(w.id).unwrap().is_none());
}

#[test]
fn healthy_tick_keeps_running_and_extends_the_lease() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let before = w.row().lease_heartbeat.unwrap();

    w.clock.sleep(TICK);

    assert!(run.tick().is_none());
    assert!(w.row().lease_heartbeat.unwrap() > before);
}

#[test]
fn serve_with_the_stop_flag_set_exits_zero_and_removes_the_row() {
    let w = World::new([]);
    w.stop.store(true, Ordering::SeqCst);
    let deps = w.deps();

    assert_eq!(serve(&deps, w.id), 0);

    assert!(w.store.proxy(w.id).unwrap().is_none());
    assert!(w.driver.dropped_tunnels().contains(&FIRST_TUNNEL_PID));
}

#[test]
fn serve_sleeps_one_tick_between_ticks_until_stopped() {
    let w = World::new([]);
    let clock = Arc::new(StopOnSleep {
        inner: w.clock.clone(),
        stop: w.stop.clone(),
        left: AtomicUsize::new(3),
    });
    let deps = w.deps_with(clock);
    let began = w.clock.now().as_millis();

    assert_eq!(serve(&deps, w.id), 0);

    let slept = w.clock.now().as_millis() - began;
    let tick = i64::try_from(TICK.as_millis()).unwrap();
    assert!(slept >= 3 * tick, "slept {slept}ms");
    assert_eq!(slept % tick, 0);
    assert!(w.store.proxy(w.id).unwrap().is_none());
}

#[test]
fn lost_lease_exits_one_and_leaves_the_new_owners_row_untouched() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.clock.sleep(LEASE);
    let taken = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(taken, Lease::Claimed));
    let stop = run.tick();

    assert!(matches!(stop, Some(Stop::LostLease)));
    assert_eq!(run.finish(Stop::LostLease), 1);
    let row = w.row();
    assert_eq!(row.lease_owner.as_deref(), Some("other-owner"));
    assert!(row.ready);
}

#[test]
fn dead_tunnel_releases_the_lease_keeps_the_row_and_exits_one() {
    let w = World::new(unrestartable());
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let stop = run.tick();

    assert!(matches!(stop, Some(Stop::TunnelDead)));
    assert_eq!(run.finish(Stop::TunnelDead), 1);
    let row = w.row();
    assert_eq!(row.lease_owner, None);
    assert!(w.stderr().to_lowercase().contains("tunnel"));
}

#[test]
fn failed_stop_releases_the_lease_and_exits_one() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert_eq!(run.finish(Stop::Failed), 1);

    assert_eq!(w.row().lease_owner, None);
}

#[test]
fn nonce_never_reaches_the_log_or_stderr_on_a_full_run() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.clock.sleep(TICK);
    assert!(run.tick().is_none());
    assert_eq!(run.finish(Stop::Signal), 0);

    assert!(!w.stderr().contains(NONCE_TEXT));
    assert!(!w.log().contains(NONCE_TEXT));
}

#[test]
fn nonce_never_reaches_the_log_or_stderr_when_the_tunnel_fails() {
    let w = World::new([TunnelReply::EarlyExit {
        code: 1,
        stderr: "boom".into(),
    }]);
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(!w.stderr().contains(NONCE_TEXT));
    assert!(!w.log().contains(NONCE_TEXT));
}

#[test]
fn missing_box_is_refused_with_a_stderr_line() {
    let w = World::new([]);
    let deps = w.deps();
    let missing = BoxId::new(9_999);

    assert_eq!(start(&deps, missing).err(), Some(1));

    assert!(!w.stderr().trim().is_empty());
    assert!(w.driver.tunnels().is_empty());
}

#[test]
fn closed_box_is_refused_with_a_stderr_line() {
    let w = World::new([]);
    w.close_box();
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(!w.stderr().trim().is_empty());
    assert!(w.driver.tunnels().is_empty());
}

#[test]
fn missing_proxy_row_is_refused_with_a_stderr_line() {
    let w = World::new([]);
    w.store.remove_proxy(w.id).unwrap();
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(!w.stderr().trim().is_empty());
    assert!(w.driver.tunnels().is_empty());
}

#[test]
fn box_without_a_box_handle_is_refused_with_a_stderr_line() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("bare").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let nonce = Secret::new(NONCE.to_vec());
    drop(
        store
            .reserve(id, &nonce, &FakeRng::default(), LEASE)
            .unwrap(),
    );
    let mut w = World::new([]);
    w.dir = dir;
    w.store = store;
    w.id = id;
    let deps = w.deps();

    assert_eq!(start(&deps, id).err(), Some(1));

    assert!(!w.stderr().trim().is_empty());
    assert!(w.driver.tunnels().is_empty());
}

/// The first tunnel comes up; every restart attempt fails.
pub(super) fn unrestartable() -> Vec<TunnelReply> {
    let fail = TunnelReply::EarlyExit {
        code: 1,
        stderr: "refused".into(),
    };
    std::iter::once(TunnelReply::Ready)
        .chain(std::iter::repeat_n(fail, 5))
        .collect()
}

#[test]
fn missing_spec_is_refused_before_the_lease_is_claimed() {
    let w = World::new([]);
    std::fs::remove_file(ca_dir(w.dir.path(), w.id).join("spec.json")).unwrap();
    let deps = w.deps();

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(w.stderr().contains("no proxy spec"));
    assert_eq!(w.row().lease_owner, None);
    assert!(w.driver.tunnels().is_empty());
}

#[test]
fn lost_lease_stops_the_proxy_from_accepting() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let port = run.host_port();
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok());
    w.clock.sleep(LEASE);
    let taken = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(taken, Lease::Claimed));
    assert!(matches!(run.tick(), Some(Stop::LostLease)));
    assert_eq!(run.finish(Stop::LostLease), 1);

    let refused = (0..100).any(|_| {
        let down = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err();
        if !down {
            std::thread::sleep(Duration::from_millis(50));
        }
        down
    });
    assert!(refused, "proxy port {port} still accepts");
}

#[test]
fn closed_removes_the_ca_and_spec() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let dir = ca_dir(w.dir.path(), w.id);
    assert!(dir.exists());

    w.close_box();
    assert!(matches!(run.tick(), Some(Stop::Closed)));
    assert_eq!(run.finish(Stop::Closed), 0);

    assert!(!dir.exists());
    assert!(read_spec(w.dir.path(), w.id).is_err());
}

#[test]
fn signal_keeps_the_ca_and_spec() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert_eq!(run.finish(Stop::Signal), 0);

    assert!(ca_dir(w.dir.path(), w.id).exists());
    assert!(read_spec(w.dir.path(), w.id).is_ok());
}

#[test]
fn a_tick_without_traffic_records_nothing() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.clock.sleep(TICK);

    assert!(run.tick().is_none());

    let events = w.store.events(w.id, 50).unwrap();
    let noisy = events
        .iter()
        .filter(|e| e.kind.contains("egress") || e.kind.contains("credential"));
    assert_eq!(noisy.count(), 0);
    assert!(w.log().is_empty());
    assert_eq!(run.finish(Stop::Signal), 0);
}
