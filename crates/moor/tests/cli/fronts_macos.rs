//! Fronts on macOS run under nono's Seatbelt profile: each may read the system paths, its
//! program and its `read_only` paths, write only its state dir, and dial only its own socket.
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real child processes with deadlines"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "a missing probe field or argv word fails the test"
)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moor::moor_core::controller::PROTOCOL;
use rustix::process::{kill_process, test_kill_process, Pid, Signal};
use serde_json::Value;

use super::{fresh_dir, fresh_dir_outside_tmp, Dir};

/// Tries each way a front could reach the controller or the host and prints what each got. Its
/// argv is the protocol, the test root, the moor binary and two Unix sockets listening on the
/// host; it waits for `ctl.pid` in its state dir, the controller's pid.
const PROBE: &str = include_str!("fronts_macos/probe.py");

/// Ignores SIGTERM, so only SIGKILL ends it, and says its pid.
const STUBBORN: &str = r#"
import json, os, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
print("probe", json.dumps({"pid": os.getpid()}), file=sys.stderr, flush=True)
time.sleep(600)
"#;

/// Forks a sleeper that outlives it, says the sleeper's pid, then exits 0.
const FORKING: &str = r#"
import json, os, sys, time
sleeper = os.fork()
if sleeper == 0:
    time.sleep(600)
    os._exit(0)
print("probe", json.dumps({"pid": sleeper}), file=sys.stderr, flush=True)
"#;

/// What a refused write says under Seatbelt.
const REFUSED: &str = "Operation not permitted";

/// A served controller, run from inside its state dir, whose operator socket sits outside it.
/// Dropping it drains the controller and waits for it to exit.
struct Controller {
    root: Dir,
    child: Child,
    /// Unix sockets listening on the host, in `/tmp` and in the hidden home.
    listeners: Vec<(PathBuf, UnixListener)>,
}

impl Controller {
    /// Serves the controller with the `fronts` section `fronts`, built from the test root.
    fn start(fronts: impl FnOnce(&Path, &[PathBuf]) -> String) -> Self {
        let root = fresh_dir_outside_tmp("front");
        for dir in ["state", "home", "cfg"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        fs::write(root.join("home").join("secret"), "x").unwrap();
        let tag = root.file_name().unwrap().to_string_lossy().into_owned();
        let listeners: Vec<(PathBuf, UnixListener)> = [
            PathBuf::from(format!("/tmp/moor-{tag}.sock")),
            root.join("home").join("h.sock"),
        ]
        .into_iter()
        .map(|path| {
            let listener = UnixListener::bind(&path).unwrap();
            (path, listener)
        })
        .collect();
        let sockets: Vec<PathBuf> = listeners.iter().map(|(p, _)| p.clone()).collect();
        let config = root.join("cfg").join("c.yaml");
        fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\ndrain_timeout_secs: 1\nfronts:\n{}",
                root.join("c.sock").display(),
                root.join("state").display(),
                fronts(&root, &sockets),
            ),
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_moor"))
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .current_dir(root.join("state"))
            .stdin(Stdio::null())
            .stdout(fs::File::create(root.join("state").join("controller.out")).unwrap())
            .stderr(fs::File::create(root.join("log")).unwrap())
            .spawn()
            .unwrap();
        let this = Self {
            root,
            child,
            listeners,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while this.version().is_none() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the operator socket; controller log:\n{}",
                fs::read_to_string(this.root.join("log")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        this
    }

    /// Serves a `probe` front that runs [`PROBE`] and may write `front`, and a `slack` front
    /// without a command.
    fn probe() -> Self {
        Self::probe_running(PROBE)
    }

    /// As [`Self::probe`], with `script` as the probe.
    fn probe_running(script: &str) -> Self {
        let python = Python::find();
        let ctl = Self::start(|root, sockets| {
            let program = hidden_program_and_note(root, &python.program);
            let command = serde_json::json!([
                program,
                "-S",
                "-c",
                script,
                PROTOCOL.to_string(),
                root,
                env!("CARGO_BIN_EXE_moor"),
                sockets[0],
                sockets[1],
            ]);
            format!(
                "  probe:\n    command: {command}\n    state_dir: {}\n    read_only: [{}, {}]\n  slack: {{}}\n",
                root.join("front").display(),
                root.join("home").join("shared").display(),
                python.prefix.display(),
            )
        });
        let pid = ctl.child.id().to_string();
        fs::create_dir_all(ctl.root.join("front")).unwrap();
        fs::write(ctl.root.join("front").join("ctl.pid"), pid).unwrap();
        ctl
    }

    /// The controller's log so far.
    fn log(&self) -> String {
        fs::read_to_string(self.root.join("log")).unwrap_or_default()
    }

    /// The first log line of front `ns` that starts with `prefix`, without the prefix.
    fn line(&self, ns: &str, prefix: &str) -> Option<String> {
        let prefix = format!("front {ns}: {prefix}");
        let log = self.log();
        let line = log.lines().find_map(|l| l.split_once(&prefix))?.1;
        Some(line.trim().to_owned())
    }

    /// What the probe front got.
    fn probed(&self) -> Value {
        let got = wait_for("the probe", || self.line("probe", "probe "));
        serde_json::from_str(&got).unwrap()
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
}

impl Drop for Controller {
    fn drop(&mut self) {
        for (path, _) in &self.listeners {
            let _ = fs::remove_file(path);
        }
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
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

/// The host's `python3`: its real path and the install prefix it reads its library from.
struct Python {
    program: PathBuf,
    prefix: PathBuf,
}

impl Python {
    fn find() -> Self {
        let out = Command::new("python3")
            .args([
                "-S",
                "-c",
                "import os, sys; print(os.path.realpath(sys.executable)); print(os.path.realpath(sys.base_prefix))",
            ])
            .output()
            .unwrap();
        let said = String::from_utf8(out.stdout).unwrap();
        let mut lines = said.lines().map(PathBuf::from);
        Self {
            program: lines.next().unwrap(),
            prefix: lines.next().unwrap(),
        }
    }
}

/// Writes a program the front runs and a note it may read, both under `root/home`, which the
/// sandbox hides; returns the program, which runs `python` with its arguments.
fn hidden_program_and_note(root: &Path, python: &Path) -> PathBuf {
    let program = root.join("home").join("bin").join("front");
    fs::create_dir_all(root.join("home").join("bin")).unwrap();
    fs::write(
        &program,
        format!("#!/bin/sh\nexec {} \"$@\"\n", python.display()),
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(root.join("home").join("shared")).unwrap();
    fs::write(root.join("home").join("shared").join("note"), "shared").unwrap();
    program
}

/// A sibling binary of `moor` in the target dir, such as `moor-web`.
fn sibling(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_moor")).with_file_name(name);
    assert!(path.is_file(), "build {name} first: {}", path.display());
    path
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

/// Whether process `pid` still exists.
fn alive(pid: i32) -> bool {
    test_kill_process(Pid::from_raw(pid).unwrap()).is_ok()
}

#[test]
fn a_front_writes_only_its_state_dir_and_its_tmp() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert_eq!(got["own_dir"], "writable", "{got}");
    assert_eq!(got["tmp"], "writable", "{got}");
    assert!(ctl.root.join("front").join("planted").exists());
    assert!(ctl.root.join("front").join("tmp").join("planted").exists());
    assert_eq!(got["own_meta"], "ok", "{got}");
    for refused in [
        "config",
        "state",
        "home",
        "shared_write",
        "root",
        "moor",
        "moor_dir",
        "system_tmp",
        "program_meta",
    ] {
        assert_eq!(got[refused], REFUSED, "{refused}: {got}");
    }
}

#[test]
fn a_front_reads_only_the_system_its_program_and_its_read_only_paths() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert_eq!(got["shared_read"], 6, "{got}");
    for hidden in ["home_secret", "config_read", "state_read"] {
        assert_eq!(got[hidden], "PermissionError", "{hidden}: {got}");
    }
}

#[test]
fn a_front_dials_its_own_socket_and_no_other() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert_eq!(got["own"], "answers", "{got}");
    for socket in ["operator", "other_front", "tmp_sock", "home_sock"] {
        assert_eq!(got[socket], "PermissionError", "{socket}: {got}");
    }
    assert_eq!(got["socket_chmod"], REFUSED, "{got}");
}

#[test]
fn a_front_writes_dev_null_and_no_other_device() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert_eq!(got["dev_null"], "writable", "{got}");
    assert_eq!(got["dev_zero"], REFUSED, "{got}");
}

#[test]
fn a_front_resolves_names_and_reads_the_system_certificates() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert!(got["resolv"].as_u64().is_some_and(|n| n > 0), "{got}");
    assert!(got["ca"].as_u64().is_some_and(|n| n > 0), "{got}");
    assert_eq!(got["dns"], "ok", "{got}");
}

#[test]
fn a_front_starts_in_root_and_cannot_signal_the_controller() {
    let ctl = Controller::probe();
    let got = ctl.probed();
    assert_eq!(got["cwd"], "/", "{got}");
    assert_eq!(got["signal_controller"], REFUSED, "{got}");
}

#[test]
fn a_drained_controller_leaves_no_front_process() {
    let ctl = Controller::probe();
    let pid = i32::try_from(ctl.probed()["pid"].as_i64().unwrap()).unwrap();
    assert!(alive(pid));
    drop(ctl);
    wait_for("the front to exit", || (!alive(pid)).then_some(()));
}

#[test]
fn a_killed_controller_leaves_no_front_process() {
    let mut ctl = Controller::probe();
    let pid = i32::try_from(ctl.probed()["pid"].as_i64().unwrap()).unwrap();
    ctl.child.kill().unwrap();
    ctl.child.wait().unwrap();
    wait_for("the front to die", || (!alive(pid)).then_some(()));
}

#[test]
fn a_front_that_ignores_sigterm_is_killed_after_the_grace() {
    let ctl = Controller::probe_running(STUBBORN);
    let pid = i32::try_from(ctl.probed()["pid"].as_i64().unwrap()).unwrap();
    drop(ctl);
    wait_for("the front to die", || (!alive(pid)).then_some(()));
}

#[test]
fn a_front_that_exits_leaves_no_process_it_forked() {
    let ctl = Controller::probe_running(FORKING);
    let sleeper = i32::try_from(ctl.probed()["pid"].as_i64().unwrap()).unwrap();
    let exited = wait_for("the front's exit", || ctl.line("probe", "exited with "));
    assert!(exited.starts_with("exit status: 0"), "{}", ctl.log());
    wait_for("the sleeper to die", || (!alive(sleeper)).then_some(()));
}

#[test]
fn a_shim_whose_controller_is_gone_exits_without_running_the_front() {
    let dir = fresh_dir_outside_tmp("orphan");
    let mark = dir.join("ran");
    let touch = format!("touch {}", mark.display());
    let mut gone = Command::new("true").spawn().unwrap();
    let gone_pid = gone.id().to_string();
    gone.wait().unwrap();
    let writable = dir.display().to_string();
    let shim = |parent: &str| {
        Command::new(env!("CARGO_BIN_EXE_moor"))
            .args(["front-shim", "--parent", parent, "--writable", &writable])
            .args(["--", "sh", "-c", &touch])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = shim(&gone_pid);
    assert!(!out.status.success());
    assert!(!mark.exists(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = shim(&std::process::id().to_string());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(mark.exists());
}

#[test]
fn the_check_tries_a_write_the_sandbox_refuses() {
    let dir = fresh_dir("check");
    let out = Command::new(env!("CARGO_BIN_EXE_moor"))
        .args(["front-shim", "--check"])
        .current_dir(&dir)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "sandbox enforced: a write outside the allow-list was refused"
    );
}

#[test]
fn moor_web_serves_under_the_sandbox() {
    let ctl = Controller::start(|root, _| {
        let token = root.join("web-token");
        fs::write(&token, "t0ken\n").unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let command = serde_json::json!([sibling("moor-web"), "--token-file", token]);
        format!(
            "  web:\n    command: {command}\n    state_dir: {}\n    read_only: [{}]\n",
            root.join("web").display(),
            token.display(),
        )
    });
    let url = wait_for("moor-web's address", || ctl.line("web", "http://"));
    let port = url.trim_end_matches('/').rsplit(':').next().unwrap();
    let mut http = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    write!(
        http,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer t0ken\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut said = String::new();
    http.read_to_string(&mut said).unwrap();
    assert!(said.starts_with("HTTP/1.1 200"), "{said}\n{}", ctl.log());
    assert!(
        ctl.root.join("web").join("web.db").exists(),
        "{}",
        ctl.log()
    );
}

#[test]
fn moor_slack_starts_under_the_sandbox() {
    let slack = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = slack.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let reply = r#"{"ok":true,"team_id":"T1","url":"ws://127.0.0.1:9/"}"#;
        for mut stream in slack.incoming().map_while(Result::ok) {
            let mut head = [0; 4096];
            let _ = stream.read(&mut head);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    let ctl = Controller::start(|root, _| {
        let cfg = root.join("slack-cfg");
        fs::create_dir(&cfg).unwrap();
        for (file, token) in [("app", "xapp-test"), ("bot", "xoxb-test")] {
            let path = cfg.join(file);
            fs::write(&path, token).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let config = cfg.join("slack.yaml");
        fs::write(
            &config,
            format!(
                "app_token_file: {}\nbot_token_file: {}\napi_base: http://127.0.0.1:{port}\n",
                cfg.join("app").display(),
                cfg.join("bot").display(),
            ),
        )
        .unwrap();
        let command = serde_json::json!([sibling("moor-slack"), "--config", config]);
        format!(
            "  slack:\n    command: {command}\n    state_dir: {}\n    read_only: [{}]\n",
            root.join("slack").display(),
            cfg.display(),
        )
    });
    wait_for("moor-slack's coordinator probe", || {
        ctl.line("slack", "moor-slack: coordinator")
    });
    assert!(
        ctl.root.join("slack").join("slack.db").exists(),
        "{}",
        ctl.log()
    );
}
