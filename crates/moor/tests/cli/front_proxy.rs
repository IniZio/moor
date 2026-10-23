//! A front whose config lists `proxy_rules` reaches the front Proxy through its shim's loopback
//! port, with a lease it got on its own socket, and trusts the CA the controller shows it. The
//! controller restarts a killed Proxy, and the same lease works again.
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
}

impl Served {
    fn start() -> Self {
        let root = fresh_dir_outside_tmp("front-proxy");
        for dir in ["state", "home", "front", "cfg"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        let command = serde_json::json!(["python3", "-c", CLIENT, PROTOCOL.to_string()]);
        let config = root.join("cfg").join("c.yaml");
        fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nfronts:\n  probe:\n    command: {command}\n    state_dir: {}\n    proxy_rules: [claude]\n",
                root.join("c.sock").display(),
                root.join("state").display(),
                root.join("front").display(),
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
        Self { root, child }
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
        let prefix = format!("front probe: {prefix} ");
        self.log()
            .lines()
            .filter_map(|l| Some(l.split_once(&prefix)?.1.trim().to_owned()))
            .collect()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.child.id()).ok().and_then(Pid::from_raw) {
            let _ = kill_process(pid, Signal::TERM);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    wait_until("a tunnel", &served, || {
        served.said("connect").iter().any(|c| c == "tls ok")
    });
    assert_eq!(served.said("stranger"), ["status 403"], "{}", served.log());
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
