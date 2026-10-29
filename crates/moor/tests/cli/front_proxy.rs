//! A front whose config lists `proxy_rules` reaches the front Proxy through its shim's loopback
//! port, with a lease it got on its own socket, and trusts the CA the controller hands it; it
//! cannot reach the Proxy socket through what pid 1 holds open. The
//! controller restarts a killed Proxy, and the same lease works again. A front the controller
//! does not start reaches the same Proxy under `moor front-run`, given only its own directory.
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real child processes with deadlines"
)]

use std::fs;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moor::moor_core::controller::PROTOCOL;
use rustix::process::{kill_process, Pid, Signal};

use super::{fresh_dir_outside_tmp, Dir};

/// Leases the Proxy as `probe:u1`, then tries a stranger's password once and its lease every
/// 0.3 s, printing what each got.
const CLIENT: &str = include_str!("front_proxy/client.py");

/// A served controller with one `probe` front whose Proxy serves the claude rule.
struct Served {
    root: Dir,
    child: Child,
    /// `moor front-run` running the front, when the controller does not start it.
    outside: Option<Child>,
}

impl Served {
    /// The controller, starting the front itself.
    fn start() -> Self {
        let command = serde_json::json!(["python3", "-c", CLIENT, PROTOCOL.to_string()]);
        Self::serve(&format!(
            "    command: {command}\n    state_dir: {{front}}\n"
        ))
    }

    /// The controller, with the front left to `moor front-run`, which gets only the front's
    /// directory.
    fn start_outside() -> Self {
        let mut served = Self::serve("");
        let dir = served.root.join("state/fronts/probe");
        wait_until("the front's CA", &served, || dir.join("ca.pem").exists());
        served.outside = Some(
            Command::new(env!("CARGO_BIN_EXE_moor"))
                .arg("front-run")
                .arg("--dir")
                .arg(&dir)
                .args(["--", "python3", "-c", CLIENT, &PROTOCOL.to_string()])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .stdin(Stdio::null())
                .stdout(fs::File::create(served.root.join("front-log")).unwrap())
                .spawn()
                .unwrap(),
        );
        served
    }

    fn serve(front: &str) -> Self {
        let root = fresh_dir_outside_tmp("front-proxy");
        for dir in ["state", "home", "front", "cfg"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        let front = front.replace("{front}", &root.join("front").display().to_string());
        let config = root.join("cfg").join("c.yaml");
        fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nfronts:\n  probe:\n{front}    proxy_rules: [claude]\n",
                root.join("c.sock").display(),
                root.join("state").display(),
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
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("log")).unwrap())
            .spawn()
            .unwrap();
        Self {
            root,
            child,
            outside: None,
        }
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("log")).unwrap_or_default()
    }

    /// Each pid the controller logged starting the Proxy with, in order.
    fn proxy_pids(&self) -> Vec<i32> {
        self.log()
            .lines()
            .filter_map(|l| {
                l.split_once("front proxy: started pid ")?
                    .1
                    .trim()
                    .parse()
                    .ok()
            })
            .collect()
    }

    /// The front's lines that start with `prefix`, without it.
    fn said(&self, prefix: &str) -> Vec<String> {
        let (lines, prefix) = if self.outside.is_some() {
            let lines = fs::read_to_string(self.root.join("front-log")).unwrap_or_default();
            (lines, format!("{prefix} "))
        } else {
            (self.log(), format!("front probe: {prefix} "))
        };
        lines
            .lines()
            .filter_map(|l| Some(l.split_once(&prefix)?.1.trim().to_owned()))
            .collect()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(outside) = &mut self.outside {
            stop(outside);
        }
        stop(&mut self.child);
    }
}

/// Sends `child` SIGTERM, which `moor front-run` passes on to its front, and kills it after 15 s.
fn stop(child: &mut Child) {
    if let Some(pid) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, Signal::TERM);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Polls `check` until it holds, for up to 30 s.
fn wait_until(what: &str, served: &Served, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}:\n{}",
            served.log()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_front_reaches_its_proxy_with_a_lease_and_a_killed_proxy_comes_back() {
    let served = Served::start();
    wait_until("a lease", &served, || !served.said("lease").is_empty());
    assert_eq!(served.said("lease"), ["granted"], "{}", served.log());
    assert!(
        !served.log().contains("front probe: exited"),
        "the front starts once: {}",
        served.log()
    );
    wait_until("a tunnel", &served, || {
        served.said("connect").iter().any(|c| c == "tls ok")
    });
    assert_eq!(served.said("stranger"), ["status 403"], "{}", served.log());
    assert_eq!(
        served.said("pid1"),
        ["PermissionError none"],
        "pid 1's open socket dir is out of the front's reach: {}",
        served.log()
    );
    let first = served.proxy_pids();
    assert_eq!(first.len(), 1, "{}", served.log());
    let pid = first.first().copied().and_then(Pid::from_raw).unwrap();
    kill_process(pid, Signal::KILL).unwrap();
    wait_until("a restarted Proxy", &served, || {
        served.proxy_pids().len() == 2
    });
    let before = served.said("connect").len();
    wait_until("a tunnel through the new Proxy", &served, || {
        served
            .said("connect")
            .iter()
            .skip(before)
            .any(|c| c == "tls ok")
    });
    assert!(!served.log().contains("never-shown"), "{}", served.log());
}

#[test]
fn a_front_the_controller_does_not_start_reaches_its_proxy_through_front_run() {
    let served = Served::start_outside();
    wait_until("a lease", &served, || !served.said("lease").is_empty());
    assert_eq!(served.said("lease"), ["granted"], "{}", served.log());
    wait_until("a tunnel", &served, || {
        served.said("connect").iter().any(|c| c == "tls ok")
    });
    assert_eq!(served.said("stranger"), ["status 403"], "{}", served.log());
    let mut held: Vec<String> = fs::read_dir(served.root.join("state/fronts/probe"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    held.sort();
    assert_eq!(held, ["ca.pem", "front", "proxy"]);
    assert!(!served.log().contains("never-shown"), "{}", served.log());
}
