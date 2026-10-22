//! Fronts run under `moor front-shim`: each reaches its own socket on fd 3 and nothing else of
//! the controller's.
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real child processes with deadlines"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "a missing probe field or argv word fails the test"
)]

use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moor::moor_core::controller::PROTOCOL;
use rustix::process::{kill_process, Pid, Signal};
use serde_json::Value;

use super::{fresh_dir, Dir};

/// Tries each way a front could reach the controller and prints what each got. Its argv is the
/// protocol and the test root; it waits for `ctl.pid` there, the controller's pid.
const PROBE: &str = r#"
import ctypes, json, os, socket, sys, time
protocol, root = int(sys.argv[1]), sys.argv[2]
def dial(path):
    try:
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(5)
        s.connect(path)
        hello = {"frame": "hello", "protocol": protocol}
        ask = {"frame": "request", "id": "1", "request": {"cmd": "version"}}
        s.sendall((json.dumps(hello) + "\n" + json.dumps(ask) + "\n").encode())
        for line in s.makefile():
            if json.loads(line)["frame"] == "response":
                return "answers"
        return "closed"
    except OSError as e:
        return type(e).__name__
def read(path):
    try:
        return len(open(path, "rb").read())
    except OSError as e:
        return type(e).__name__
def listdir(path):
    try:
        return len(os.listdir(path))
    except OSError as e:
        return type(e).__name__
while not os.path.exists(f"{root}/ctl.pid"):
    time.sleep(0.05)
ctl = open(f"{root}/ctl.pid").read().strip()
libc = ctypes.CDLL(None, use_errno=True)
unmounted = libc.umount2(f"{root}/state".encode(), 2) == 0
got = {
    "own": dial("/proc/self/fd/3"),
    "operator": dial(f"{root}/c.sock"),
    "other_front": dial(f"{root}/state/fronts/slack.sock"),
    "vault_key": read(f"{root}/vault.key"),
    "controller_fds": listdir(f"/proc/{ctl}/fd"),
    "controller_root": dial(f"/proc/{ctl}/root{root}/c.sock"),
    "unmount": "allowed" if unmounted else os.strerror(ctypes.get_errno()),
}
print("probe", json.dumps(got), file=sys.stderr, flush=True)
time.sleep(600)
"#;

/// Leaves an orphan for pid 1 to reap, reports whether it was, then exits 7 on SIGTERM.
const ORPHANING: &str = r#"
import os, signal, sys, time
def term(*_):
    print("front got TERM", file=sys.stderr, flush=True)
    sys.exit(7)
signal.signal(signal.SIGTERM, term)
r, w = os.pipe()
if os.fork() == 0:
    orphan = os.fork()
    if orphan == 0:
        time.sleep(0.3)
        os._exit(0)
    os.write(w, str(orphan).encode())
    os._exit(0)
orphan = int(os.read(r, 32))
os.wait()
state, deadline = "gone", time.time() + 10
while time.time() < deadline:
    try:
        state = open(f"/proc/{orphan}/stat").read().rsplit(")", 1)[1].split()[0]
    except OSError:
        state = "gone"
        break
    time.sleep(0.05)
print("orphan", state, file=sys.stderr, flush=True)
while True:
    time.sleep(1)
"#;

/// Runs a moor binary in a user namespace where no further user namespace can be created.
const NO_USER_NAMESPACES: [&str; 6] = [
    "unshare",
    "--user",
    "--map-root-user",
    "sh",
    "-c",
    r#"echo 0 > /proc/sys/user/max_user_namespaces && exec "$0" "$@""#,
];

/// A served controller whose operator socket sits outside its state dir, with a `probe` front
/// that runs a Python script and a `slack` front without a command. Dropping it drains the
/// controller and waits for it to exit.
struct Controller {
    root: Dir,
    child: Child,
}

impl Controller {
    /// Serves the controller with `script` as the probe front, prefixing the moor command with
    /// `wrap`.
    fn start(script: &str, wrap: &[&str]) -> Self {
        let root = fresh_dir("front");
        let config = root.join("c.yaml");
        let command = serde_json::json!(["python3", "-c", script, PROTOCOL.to_string(), &*root]);
        fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nfronts:\n  probe:\n    command: {command}\n  slack: {{}}\n",
                root.join("c.sock").display(),
                root.join("state").display(),
            ),
        )
        .unwrap();
        let moor = env!("CARGO_BIN_EXE_moor");
        let argv: Vec<&str> = wrap.iter().copied().chain([moor]).collect();
        let child = Command::new(argv[0])
            .args(&argv[1..])
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("log")).unwrap())
            .spawn()
            .unwrap();
        fs::write(root.join("ctl.pid"), child.id().to_string()).unwrap();
        let this = Self { root, child };
        wait_for("the operator socket", || this.version());
        this
    }

    /// The controller's log so far.
    fn log(&self) -> String {
        fs::read_to_string(self.root.join("log")).unwrap_or_default()
    }

    /// The first log line of the probe front that starts with `prefix`, without the prefix.
    fn line(&self, prefix: &str) -> Option<String> {
        let prefix = format!("front probe: {prefix}");
        let log = self.log();
        let line = log.lines().find_map(|l| l.split_once(&prefix))?.1;
        Some(line.trim().to_owned())
    }

    /// Whether `version` answers on the operator socket.
    fn version(&self) -> Option<()> {
        let mut s = UnixStream::connect(self.root.join("c.sock")).ok()?;
        let hello = serde_json::json!({"frame": "hello", "protocol": PROTOCOL});
        let ask = serde_json::json!({"frame": "request", "id": "1", "request": {"cmd": "version"}});
        writeln!(s, "{hello}\n{ask}").ok()?;
        BufReader::new(s)
            .lines()
            .map_while(Result::ok)
            .find_map(|l| {
                let v: Value = serde_json::from_str(&l).ok()?;
                (v["frame"] == "response").then_some(())
            })
    }

    fn pid_file(&self) -> PathBuf {
        self.root.join("state").join("fronts").join("probe.pid")
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let pid = Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        let _ = kill_process(pid, Signal::TERM);
        let deadline = Instant::now() + Duration::from_secs(15);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// The pids whose command line names `marker`.
fn processes_naming(marker: &str) -> Vec<String> {
    fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let cmdline = fs::read(format!("/proc/{name}/cmdline")).ok()?;
            String::from_utf8_lossy(&cmdline)
                .contains(marker)
                .then_some(name)
        })
        .collect()
}

#[test]
fn a_front_reaches_its_own_socket_and_nothing_else_of_the_controller() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["own"], "answers", "{got}");
    assert_eq!(got["operator"], "ConnectionRefusedError", "{got}");
    assert_eq!(got["other_front"], "FileNotFoundError", "{got}");
    assert_eq!(got["vault_key"], 0, "{got}");
    assert_eq!(got["controller_fds"], "FileNotFoundError", "{got}");
    assert_eq!(got["controller_root"], "FileNotFoundError", "{got}");
    assert_eq!(got["unmount"], "Operation not permitted", "{got}");
}

#[test]
fn the_shim_reaps_orphans_and_passes_sigterm_to_the_front() {
    let ctl = Controller::start(ORPHANING, &[]);
    assert_eq!(wait_for("the orphan", || ctl.line("orphan ")), "gone");
    let shim: i32 = fs::read_to_string(ctl.pid_file())
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    kill_process(Pid::from_raw(shim).unwrap(), Signal::TERM).unwrap();
    wait_for("the front's TERM", || ctl.line("front got TERM"));
    let exited = wait_for("the front's exit", || ctl.line("exited with "));
    assert!(exited.starts_with("exit status: 7;"), "{}", ctl.log());
}

#[test]
fn a_drained_controller_leaves_no_front_process() {
    let ctl = Controller::start(PROBE, &[]);
    wait_for("the probe", || ctl.line("probe "));
    let root = ctl.root.display().to_string();
    assert!(!processes_naming(&root).is_empty());
    drop(ctl);
    wait_for("every front process to exit", || {
        processes_naming(&root).is_empty().then_some(())
    });
}

#[test]
fn without_user_namespaces_no_front_starts_and_the_controller_serves() {
    let ctl = Controller::start(PROBE, &NO_USER_NAMESPACES);
    let log = wait_for("the refusal", || {
        Some(ctl.log()).filter(|l| l.contains("fronts not started"))
    });
    assert!(log.contains("user.max_user_namespaces"), "{log}");
    assert!(log.contains("No space left on device"), "{log}");
    assert!(!log.contains("front probe"), "{log}");
    assert!(!ctl.pid_file().exists());
    assert_eq!(ctl.version(), Some(()));
}
