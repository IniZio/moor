//! The CLI routes every verb through the controller when one is configured.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use super::fresh_dir;

/// A running controller and the paths a client needs, torn down on drop.
pub(crate) struct Daemon {
    pub(crate) root: PathBuf,
    pub(crate) socket: PathBuf,
    child: Child,
}

impl Daemon {
    pub(crate) fn start() -> Self {
        Self::start_with(|_| {})
    }

    /// [`Self::start`], after `configure` adjusts the daemon's command.
    pub(crate) fn start_with(configure: impl FnOnce(&mut Command)) -> Self {
        Self::start_in(fresh_dir("via"), configure)
    }

    /// [`Self::start_with`] in `root`, so a client can be pointed at the socket first.
    fn start_in(root: PathBuf, configure: impl FnOnce(&mut Command)) -> Self {
        // Kept short: Unix socket paths are limited to about 104 bytes.
        let socket = root.join("c.sock");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
                socket.display(),
                root.join("state").display()
            ),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MOOR_STATE_DIR", root.join("state"))
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure(&mut cmd);
        let child = cmd.spawn().unwrap();
        let this = Self {
            root,
            socket,
            child,
        };
        // 500 polls of 10ms bound the wait at about five seconds.
        for _ in 0..500 {
            if UnixStream::connect(&this.socket).is_ok() {
                return this;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("controller never listened");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Exit code and stdout of one client run.
struct Out {
    code: i32,
    stdout: String,
}

/// Runs the binary with a hermetic environment plus `envs`.
fn moor(home: &Path, envs: &[(&str, &Path)], args: &[&str]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .args(args)
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    Out {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
    }
}

fn envelope(out: &Out) -> Value {
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", out.stdout))
}

fn class(v: &Value) -> &str {
    v.get("class").and_then(Value::as_str).unwrap()
}

/// Writes a config the local path cannot load.
fn broken_config(dir: &Path) -> PathBuf {
    let path = dir.join("broken.yaml");
    std::fs::write(&path, "tools: [unterminated\n  : : {").unwrap();
    path
}

#[test]
fn env_socket_routes_show_to_the_daemon() {
    let daemon = Daemon::start();
    let home = daemon.root.join("client-home");
    let broken = broken_config(&daemon.root);
    let envs = [("MOOR_CONFIG", broken.as_path())];
    let local = moor(&home, &envs, &["--json", "show"]);
    assert_ne!(class(&envelope(&local)), "ok", "control must fail locally");

    let envs = [
        ("MOOR_CONFIG", broken.as_path()),
        ("MOOR_CONTROLLER", daemon.socket.as_path()),
    ];
    let remote = moor(&home, &envs, &["--json", "show"]);
    assert_eq!(remote.code, 0, "{}", remote.stdout);
    let state = daemon.root.join("state");
    let direct = moor(
        &home,
        &[("MOOR_STATE_DIR", state.as_path())],
        &["--json", "show"],
    );
    assert_eq!(envelope(&remote), envelope(&direct));
}

#[test]
fn config_socket_routes_show_to_the_daemon() {
    let daemon = Daemon::start();
    let home = daemon.root.join("client-home");
    let config = daemon.root.join("client.yaml");
    std::fs::write(
        &config,
        format!("controller:\n  socket: {}\n", daemon.socket.display()),
    )
    .unwrap();
    // A state dir that is a plain file makes any in-process run fail.
    let bogus = daemon.root.join("not-a-dir");
    std::fs::write(&bogus, "x").unwrap();
    let envs = [
        ("MOOR_CONFIG", config.as_path()),
        ("MOOR_STATE_DIR", bogus.as_path()),
    ];
    let remote = moor(&home, &envs, &["--json", "show"]);
    assert_eq!(remote.code, 0, "{}", remote.stdout);
    assert_eq!(class(&envelope(&remote)), "ok");
}

#[test]
fn unreachable_controller_fails_without_local_fallback() {
    let root = fresh_dir("via-down");
    let missing = root.join("missing.sock");
    let envs = [
        ("MOOR_CONTROLLER", missing.as_path()),
        ("MOOR_CONTROLLER_REDIAL_SECS", Path::new("0")),
    ];
    for verb in ["version", "show"] {
        let out = moor(&root, &envs, &["--json", verb]);
        assert_eq!(out.code, 1, "{verb}: {}", out.stdout);
        let v = envelope(&out);
        assert_eq!(class(&v), "failed");
        let hints = v
            .get("error")
            .and_then(|e| e.get("hints"))
            .unwrap()
            .to_string();
        assert!(hints.contains("moor controller serve"), "{hints}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn exit_codes_match_local_mode() {
    let daemon = Daemon::start();
    let home = daemon.root.join("client-home");
    let state = daemon.root.join("state");
    let cases: [&[&str]; 2] = [
        &["--json", "run", "foo", "--", "true"],
        &["--json", "start", "BAD_Name"],
    ];
    for args in cases {
        let local = moor(&home, &[("MOOR_STATE_DIR", state.as_path())], args);
        let envs = [("MOOR_CONTROLLER", daemon.socket.as_path())];
        let remote = moor(&home, &envs, args);
        assert_eq!(remote.code, local.code, "{args:?}: {}", remote.stdout);
        assert_eq!(
            class(&envelope(&remote)),
            class(&envelope(&local)),
            "{args:?}"
        );
    }
}

#[test]
fn a_request_sent_to_a_stale_socket_is_answered_once_a_controller_binds_it() {
    let root = fresh_dir("via-late");
    let socket = root.join("c.sock");
    drop(UnixListener::bind(&socket).unwrap());
    let client = Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("client-home"))
        .env("MOOR_CONTROLLER", &socket)
        .args(["--json", "show"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let _daemon = Daemon::start_in(root, |_| {});
    let out = client.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(class(&v), "ok");
}
