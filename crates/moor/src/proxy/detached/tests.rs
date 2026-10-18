//! Tests for the detached proxy supervisor adapter and its helpers.

mod spec;
mod stop;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::{ProxyHost, ProxyHostError, ProxySpec, ProxyStatus};
use moor_core::{BoxId, BoxName, Secret};

use super::{base64url, log_path, new_nonce, tail, DetachedSupervisor, LEASE, TAIL_LINES, WAIT};
use crate::store::{Fenced, ProxyPorts, ReadyProc, Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::proc::{DaemonCall, ScriptedProc};
use crate::testing::rng::FakeRng;

const HOST_PORT: u16 = 41000;

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
    /// The Task every test works on.
    pub(super) id: BoxId,
}

/// Simulates a healthy supervisor for each spawned daemon: it takes the lease on the
/// task named in the daemon's arguments and marks the proxy ready.
pub(super) fn supervise(state: PathBuf) -> impl Fn(&DaemonCall) + Send + Sync + 'static {
    move |d| {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        let store = Store::open(&state.join(STORE_FILE), clock).unwrap();
        let task = BoxId::new(d.call.args.get(2).unwrap().parse().unwrap());
        let owner = "fake-supervisor";
        store.claim_lease(task, owner, LEASE).unwrap();
        let row = store.proxy(task).unwrap().unwrap();
        let ports = ProxyPorts {
            box_port: row.box_port,
            host_port: HOST_PORT,
        };
        let applied = store
            .mark_ready(
                task,
                owner,
                ports,
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
        .apply(Decision::insert(NewTask {
            name: BoxName::parse("t").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
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
            task: self.id,
            box_handle: None,
            host_cwd: PathBuf::from("/work"),
            task_repo: None,
            rules: Vec::new(),
        }
    }

    /// Reserve a row and let a supervisor named `sup` hold its lease now.
    pub(super) fn leased_row(&self) {
        let nonce = new_nonce(self.rng.as_ref());
        let _ = self
            .store
            .reserve(self.id, &nonce, self.rng.as_ref(), LEASE)
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

    /// Whether the Task still has a proxy row.
    pub(super) fn has_row(&self) -> bool {
        self.store.proxy(self.id).unwrap().is_some()
    }

    /// The nonce stored on the Task's row, as text.
    fn nonce_text(&self) -> String {
        let row = self.store.proxy(self.id).unwrap().unwrap();
        String::from_utf8(row.nonce.expose().to_vec()).unwrap()
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
    // The directory holding every Task's spec, CA and log is owner-only.
    let mode = std::fs::metadata(d.log.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let row = f.store.proxy(f.id).unwrap().unwrap();
    assert!(row.ready);
    assert_eq!(ep.box_port, row.box_port);
    assert_eq!(ep.nonce.expose(), row.nonce.expose());
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
    assert_eq!(first.nonce.expose(), second.nonce.expose());
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

#[test]
fn the_nonce_stays_out_of_daemon_arguments_environment_and_log() {
    let f = fx();
    f.sup.ensure(&f.spec()).unwrap();
    let nonce = f.nonce_text();
    assert!(!nonce.is_empty());
    let daemons = f.proc.daemons();
    let d = daemons.first().unwrap();
    let mut seen = vec![format!("{:?}", d.call.args), format!("{:?}", d.call.env)];
    for call in f.proc.calls() {
        seen.push(format!("{:?} {:?}", call.args, call.env));
    }
    seen.push(std::fs::read_to_string(&d.log).unwrap_or_default());
    for text in seen {
        assert!(!text.contains(&nonce), "{text}");
    }

    let g = bare();
    let err = g.sup.ensure(&g.spec()).unwrap_err();
    assert!(!err.to_string().contains(&g.nonce_text()));
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
    f.exec("UPDATE proxy SET ready = 1, host_port = 4000");
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
    f.exec("UPDATE proxy SET ready = 1, host_port = 4000");
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

/// A nonce is 32 bytes of base64url text: 43 characters.
#[test]
fn nonce_is_43_url_safe_bytes() {
    let nonce: Secret = new_nonce(&FakeRng::default());
    let bytes = nonce.expose();
    assert_eq!(bytes.len(), 43);
    assert!(
        bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_'),
        "unexpected byte in nonce"
    );
}

/// Successive draws from one source yield different nonces.
#[test]
fn nonces_differ_across_draws() {
    let rng = FakeRng::default();
    let a = new_nonce(&rng);
    let b = new_nonce(&rng);
    assert_ne!(a.expose(), b.expose());
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
