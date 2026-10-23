//! Fronts the controller runs as its children, each dialing its own socket on fd 3.
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real child processes with deadlines"
)]

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::controller::PROTOCOL;
use rustix::process::{kill_process, Pid, Signal};

use super::{config, env, Buf};
use crate::controller::{fronts::Supervised, start, Sink};

/// Asks `version` on `MOOR_FRONT_SOCKET`, prints the answer to stderr, then idles.
const FAKE_FRONT: &str = r#"
import json, os, socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ["MOOR_FRONT_SOCKET"])
hello = {"frame": "hello", "protocol": int(sys.argv[1])}
ask = {"frame": "request", "id": "1", "request": {"cmd": "version"}}
s.sendall((json.dumps(hello) + "\n" + json.dumps(ask) + "\n").encode())
for line in s.makefile():
    got = json.loads(line)
    if got["frame"] == "response":
        print("version", got["envelope"]["class"], json.dumps(got["envelope"]["value"]), file=sys.stderr, flush=True)
        break
time.sleep(600)
"#;

/// Stands in for `moor front-shim` without namespaces: runs the command after `--`.
static PASS_THROUGH_SHIM: std::sync::LazyLock<(OsString, Vec<OsString>)> =
    std::sync::LazyLock::new(|| {
        let script = r#"case "$1" in --check) exit 0;; esac
while [ "$1" != -- ]; do shift; done
shift
exec "$@""#;
        (
            "sh".into(),
            vec!["-c".into(), script.into(), "front-shim".into()],
        )
    });

/// A served controller whose `web` front runs [`FAKE_FRONT`] and whose `slack` front has no
/// command. Dropping it stops the fronts before the state dir goes.
struct Fronts {
    supervised: Option<Supervised>,
    log: Buf,
    dir: tempfile::TempDir,
}

/// Stands in for `moor front-shim` on a host without the sandbox: `--check` fails.
fn refusing_shim() -> (OsString, Vec<OsString>) {
    let script = r#"echo "they need nono's sandbox, which this linux host lacks" >&2; exit 1"#;
    (
        "sh".into(),
        vec!["-c".into(), script.into(), "front-shim".into()],
    )
}

impl Fronts {
    fn start() -> Self {
        Self::start_with(PASS_THROUGH_SHIM.clone())
    }

    fn start_with(shim: (OsString, Vec<OsString>)) -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        let cfg = config(
            dir.path(),
            &format!(
                "fronts:\n  web:\n    command: [python3, -c, {}, '{PROTOCOL}']\n  slack: {{}}\n",
                serde_json::to_string(FAKE_FRONT).expect("quote")
            ),
        );
        let mut running = start(&env(dir.path()), &cfg).expect("start");
        let log = Buf::default();
        let sink: Sink = Arc::new(Mutex::new(log.clone()));
        let supervised = Some(running.supervise_fronts(shim, &sink));
        running.serve_in_background(sink);
        Self {
            supervised,
            log,
            dir,
        }
    }

    fn pid_file(&self, ns: &str) -> PathBuf {
        self.dir
            .path()
            .join("state")
            .join("fronts")
            .join(format!("{ns}.pid"))
    }

    /// The pid and start time the pid file of `ns` names, once it exists.
    fn recorded(&self, ns: &str) -> Option<(u32, String)> {
        let text = fs::read_to_string(self.pid_file(ns)).ok()?;
        let mut lines = text.lines();
        Some((lines.next()?.parse().ok()?, lines.next()?.to_owned()))
    }

    fn answers(&self) -> usize {
        self.log.text().matches("front web: version ok").count()
    }
}

impl Drop for Fronts {
    fn drop(&mut self) {
        drop(self.supervised.take());
    }
}

/// Polls `check` until it returns something, for up to 20 s.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(got) = check() {
            return got;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The start time `/proc` gives for `pid`, in clock ticks after boot.
fn start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19).map(str::to_owned)
}

#[test]
fn a_front_answers_through_the_socket_on_its_fd_3() {
    let fronts = Fronts::start();
    wait_for("an answer", || (fronts.answers() == 1).then_some(()));
    let log = fronts.log.text();
    assert!(log.contains(env!("CARGO_PKG_VERSION")), "{log}");
}

#[test]
fn the_pid_file_names_the_live_front() {
    let fronts = Fronts::start();
    let (pid, started) = wait_for("a pid file", || fronts.recorded("web"));
    assert_eq!(start_time(pid), Some(started));
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).expect("cmdline");
    assert!(String::from_utf8_lossy(&cmdline).contains("MOOR_FRONT_SOCKET"));
}

#[test]
fn a_killed_front_is_started_again() {
    let fronts = Fronts::start();
    wait_for("an answer", || (fronts.answers() == 1).then_some(()));
    let (first, _) = fronts.recorded("web").expect("pid file");
    kill_process(
        Pid::from_raw(i32::try_from(first).expect("pid")).expect("pid"),
        Signal::KILL,
    )
    .expect("kill the front");
    wait_for("a second answer", || (fronts.answers() == 2).then_some(()));
    let (second, started) = wait_for("a new pid file", || {
        fronts.recorded("web").filter(|(pid, _)| *pid != first)
    });
    assert_eq!(start_time(second), Some(started));
    assert!(
        fronts.log.text().contains("restarting in"),
        "{}",
        fronts.log.text()
    );
}

#[test]
fn a_front_without_a_command_is_not_started() {
    let fronts = Fronts::start();
    wait_for("an answer", || (fronts.answers() == 1).then_some(()));
    assert!(!fronts.pid_file("slack").exists());
    assert!(
        !fronts.log.text().contains("front slack"),
        "{}",
        fronts.log.text()
    );
}

#[test]
fn stopping_the_supervision_ends_the_front() {
    let mut fronts = Fronts::start();
    let (pid, _) = wait_for("a pid file", || fronts.recorded("web"));
    drop(fronts.supervised.take());
    assert_eq!(start_time(pid), None, "front {pid} outlived its supervisor");
    assert!(!fronts.pid_file("web").exists());
}

#[test]
fn a_host_the_shim_check_refuses_starts_no_front_and_says_why() {
    let fronts = Fronts::start_with(refusing_shim());
    let log = wait_for("the refusal", || {
        Some(fronts.log.text()).filter(|l| l.contains("fronts not started"))
    });
    assert!(log.contains("nono's sandbox"), "{log}");
    assert!(!fronts.pid_file("web").exists());
    assert!(!log.contains("front web"), "{log}");
}
