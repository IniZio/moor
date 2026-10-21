//! Tests for the detached proxy supervisor adapter and its helpers.

mod spec;
mod stop;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::{ProxyHost, ProxyHostError, ProxySpec, ProxyStatus};
use moor_core::{BoxId, BoxName};

use super::{base64url, log_path, tail, DetachedSupervisor, LEASE, TAIL_LINES, WAIT};
use crate::store::{Fenced, ReadyProc, Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::proc::{DaemonCall, ScriptedProc};
use crate::testing::rng::FakeRng;

/// An adapter over fakes, with a second store on the same database to seed and inspect rows.
pub(super) struct Fx {
    /// State directory.
    pub(super) dir: tempfile::TempDir,
    /// Scripted process runner the adapter spawns through.
    pub(super) proc: Arc<ScriptedProc>,
    /// Clock shared by the adapter and the inspecting store.
    pub(super) clock: Arc<FakeClock>,
    /// Second connection to the adapter's database.
    pub(super) store: Store,
    /// Random source shared with the adapter.
    pub(super) rng: Arc<FakeRng>,
    /// The adapter under test.
    pub(super) sup: DetachedSupervisor,
    /// The Box every test works on.
    pub(super) id: BoxId,
}

/// Simulates a healthy supervisor for each spawned daemon: it takes the lease on the
/// Box named in the daemon's arguments and marks the proxy ready.
pub(super) fn supervise(state: PathBuf) -> impl Fn(&DaemonCall) + Send + Sync + 'static {
    move |d| {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        let store = Store::open(&state.join(STORE_FILE), clock).unwrap();
        let box_id = BoxId::new(d.call.args.get(2).unwrap().parse().unwrap());
        let owner = "fake-supervisor";
        store.claim_lease(box_id, owner, LEASE).unwrap();
        let row = store.proxy(box_id).unwrap().unwrap();
        let box_port = row.box_port;
        let applied = store
            .mark_ready(
                box_id,
                owner,
                box_port,
                ReadyProc {
                    pid: d.pid,
                    handle: "fake-handle",
                    boot_id: None,
                },
            )
            .unwrap();
        assert_eq!(applied, Fenced::Applied);
    }
}

/// A fixture whose daemons are healthy supervisors.
pub(super) fn fx() -> Fx {
    let f = bare();
    f.proc.on_daemon(supervise(f.dir.path().to_path_buf()));
    f
}

/// A fixture whose daemons do nothing.
pub(super) fn bare() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let rng = Arc::new(FakeRng::default());
    let store = Store::open(&dir.path().join(STORE_FILE), clock.clone()).unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let sup = DetachedSupervisor::new(
        proc.clone(),
        clock.clone(),
        rng.clone(),
        Some(dir.path().to_path_buf()),
    );
    Fx {
        dir,
        proc,
        clock,
        store,
        rng,
        sup,
        id,
    }
}

impl Fx {
    /// The spec the adapter is asked to ensure.
    pub(super) fn spec(&self) -> ProxySpec {
        ProxySpec {
            box_id: self.id,
            box_handle: None,
            box_repo: None,
            rules: Vec::new(),
            user: moor_core::user::UserId::new(1),
            vault_key: "/config/vault.key".into(),
        }
    }

    /// Reserve a row and let a supervisor named `sup` hold its lease now.
    pub(super) fn leased_row(&self) {
        let _ = self
            .store
            .reserve(self.id, self.rng.as_ref(), LEASE)
            .unwrap();
        self.store.claim_lease(self.id, "sup", LEASE).unwrap();
    }

    /// Run `sql` on a raw connection to the database.
    pub(super) fn exec(&self, sql: &str) {
        rusqlite::Connection::open(self.dir.path().join(STORE_FILE))
            .unwrap()
            .execute(sql, [])
            .unwrap();
    }

    /// Whether the Box still has a proxy row.
    pub(super) fn has_row(&self) -> bool {
        self.store.proxy(self.id).unwrap().is_some()
    }

    /// Time slept on the shared clock since `from`.
    fn elapsed(&self, from: Ts) -> Duration {
        let ms = self.clock.now().as_millis() - from.as_millis();
        Duration::from_millis(u64::try_from(ms).unwrap())
    }
}

/// Replaces the supervisor with one that prints `lines` numbered lines and never gets ready.
fn hang_after_logging(f: &Fx, lines: usize) {
    f.proc.on_daemon(move |d| {
        let mut text = String::new();
        for n in 1..=lines {
            writeln!(text, "line {n}").unwrap();
        }
        std::fs::create_dir_all(d.log.parent().unwrap()).unwrap();
        std::fs::write(&d.log, text).unwrap();
    });
}

#[test]
fn the_first_ensure_spawns_one_supervisor_and_returns_its_address() {
    let f = fx();

    let ep = f.sup.ensure(&f.spec()).unwrap();

    let daemons = f.proc.daemons();
    assert_eq!(daemons.len(), 1);
    let d = daemons.first().unwrap();
    assert_eq!(d.call.tool, Tool::MoorSelf);
    assert_eq!(d.call.args, ["proxy", "serve", &f.id.get().to_string()]);
    assert_eq!(d.log, log_path(f.dir.path(), f.id));
    // Creating the log file is the spawner's job, so it can pick the mode.
    assert!(!d.log.exists());
    // The directory holding every Box's spec, CA and log is owner-only.
    let mode = std::fs::metadata(d.log.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let row = f.store.proxy(f.id).unwrap().unwrap();
    assert!(row.ready);
    assert_eq!(ep.box_port, row.box_port);
}

#[test]
fn the_supervisor_is_told_the_state_dir_this_host_uses() {
    let f = fx();

    f.sup.ensure(&f.spec()).unwrap();

    let daemons = f.proc.daemons();
    let env = &daemons.first().unwrap().call.env;
    let dir = f.dir.path().to_string_lossy().into_owned();
    assert!(env.contains(&("MOOR_STATE_DIR".to_owned(), dir)), "{env:?}");
}

#[test]
fn a_second_ensure_reuses_the_running_supervisor() {
    let f = fx();
    let first = f.sup.ensure(&f.spec()).unwrap();

    let second = f.sup.ensure(&f.spec()).unwrap();

    assert_eq!(f.proc.daemons().len(), 1);
    assert_eq!(first.box_port, second.box_port);
}

#[test]
fn a_supervisor_that_never_gets_ready_fails_with_the_log_tail() {
    let f = bare();
    hang_after_logging(&f, 25);
    let start = f.clock.now();

    let err = f.sup.ensure(&f.spec()).unwrap_err();

    assert!(f.elapsed(start) >= WAIT);
    let ProxyHostError::Failed(message) = err else {
        unreachable!("expected Failed, got {err:?}");
    };
    let (reason, tail) = message.split_once('\n').unwrap();
    assert_eq!(reason, "the proxy supervisor was not ready after 30s");
    let expected: Vec<String> = (25 - TAIL_LINES + 1..=25)
        .map(|n| format!("line {n}"))
        .collect();
    assert_eq!(tail.lines().collect::<Vec<_>>(), expected);
    assert_eq!(expected.first().map(String::as_str), Some("line 6"));
}

#[test]
fn a_supervisor_leased_by_another_process_is_waited_for_not_spawned() {
    let f = fx();
    f.leased_row();
    let start = f.clock.now();

    let err = f.sup.ensure(&f.spec()).unwrap_err();

    assert!(matches!(err, ProxyHostError::Failed(_)));
    assert!(f.proc.daemons().is_empty());
    assert!(f.elapsed(start) >= WAIT);
}

/// The adapter and the supervisor keep separate copies of the lease; they must agree.
#[test]
fn adapter_and_supervisor_share_one_lease() {
    assert_eq!(LEASE, crate::proxy::supervisor::LEASE);
}

#[test]
fn status_is_absent_without_a_row() {
    let f = fx();
    assert_eq!(f.sup.status(f.id).unwrap(), ProxyStatus::Absent);
}

#[test]
fn status_is_starting_for_a_live_unready_row() {
    let f = fx();
    f.leased_row();
    assert_eq!(f.sup.status(f.id).unwrap(), ProxyStatus::Starting);
}

#[test]
fn status_is_ready_with_the_box_port_for_a_live_ready_row() {
    let f = fx();
    f.leased_row();
    f.exec("UPDATE proxy SET ready = 1");
    let port = f.store.proxy(f.id).unwrap().unwrap().box_port;
    assert_eq!(
        f.sup.status(f.id).unwrap(),
        ProxyStatus::Ready { box_port: port }
    );
}

#[test]
fn status_is_stale_after_the_lease_expires() {
    let f = fx();
    f.leased_row();
    f.exec("UPDATE proxy SET ready = 1");
    f.clock.sleep(LEASE + Duration::from_secs(1));
    assert_eq!(f.sup.status(f.id).unwrap(), ProxyStatus::Stale);
}

/// Encoding follows RFC 4648 section 5 without padding.
#[test]
fn base64url_known_vectors() {
    assert_eq!(base64url(b""), "");
    assert_eq!(base64url(b"f"), "Zg");
    assert_eq!(base64url(b"ab"), "YWI");
    assert_eq!(base64url(b"foo"), "Zm9v");
    assert_eq!(base64url(b"foob"), "Zm9vYg");
}

/// The URL-safe alphabet replaces `+` and `/` and never pads.
#[test]
fn base64url_uses_url_alphabet() {
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
}

/// The log lives under `<state>/proxy/<id>.log`.
#[test]
fn log_path_is_under_proxy_dir() {
    assert_eq!(
        log_path(Path::new("/s"), BoxId::new(7)),
        Path::new("/s/proxy/7.log")
    );
}

/// Only the last `TAIL_LINES` lines are returned.
#[test]
fn tail_keeps_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.log");
    let lines: Vec<String> = (1..=25).map(|i| format!("line {i}")).collect();
    let body = lines.join("\n") + "\n";
    std::fs::write(&path, body).unwrap();

    let got = tail(&path);

    let want: Vec<String> = (6..=25).map(|i| format!("line {i}")).collect();
    assert_eq!(TAIL_LINES, 20);
    assert_eq!(got.lines().collect::<Vec<_>>(), want);
}

/// An unreadable log yields an empty reason.
#[test]
fn tail_of_missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(tail(&dir.path().join("absent.log")), "");
}
