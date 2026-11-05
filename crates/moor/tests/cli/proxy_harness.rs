//! A real controller and the real Box Proxy it starts, with a fake crabbox whose Box is this
//! host: a loop of requests through the Box's Proxy port reaches a local TLS upstream while the
//! test kills processes at chosen points.

#![allow(
    clippy::disallowed_methods,
    reason = "the test times real processes with the wall clock"
)]

use std::fmt::Write as _;
use std::fs::File;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use moor::test_upstream::{seed_box, UPSTREAM, UPSTREAM_CA};
use moor_testkit::{box_client, write_fake_crabbox, Fleet, Tally, TlsUpstream, Traffic};
use rustix::process::Pid;

/// The Box user's token, which only the upstream may see.
const TOKEN: &str = "harness-token-5d1e";

/// Longest wait for a Proxy to come up; the controller first waits one lease (30 s).
const PROXY_UP: Duration = Duration::from_secs(90);

/// How long no controller runs: five of the Proxy's heartbeats.
const GAP: Duration = Duration::from_secs(10);

/// Pause between two requests of the loop.
const PAUSE: Duration = Duration::from_millis(100);

/// The Proxy row of the seeded Box once its supervisor is ready.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ready {
    box_port: u16,
    pid: i32,
    owner: String,
    ca_pem: String,
}

/// One controller config and state dir, its Box, and the upstream the Box reaches.
struct Harness {
    root: super::Dir,
    config: PathBuf,
    crabbox: PathBuf,
    upstream: TlsUpstream,
    box_id: i64,
}

impl Harness {
    fn new() -> Self {
        let root = super::fresh_dir_outside_tmp("harness");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
                root.join("c.sock").display(),
                root.join("state").display()
            ),
        )
        .unwrap();
        let crabbox = root.join("crabbox");
        write_fake_crabbox(&crabbox, &root.join("crabbox.log")).unwrap();
        let upstream = TlsUpstream::start(&root).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        let box_id = seed_box(
            &root.join("state"),
            &root.join("vault.key"),
            upstream.port,
            TOKEN,
        )
        .unwrap();
        Self {
            root,
            config,
            crabbox,
            upstream,
            box_id,
        }
    }

    /// Starts `moor controller serve` in a session of its own, logging to `controller-<n>.log`.
    fn controller(&self, fleet: &mut Fleet, n: u8) -> Pid {
        let log = File::create(self.root.join(format!("controller-{n}.log"))).unwrap();
        let mut cmd = Fleet::command(env!("CARGO_BIN_EXE_moor"));
        cmd.args(["controller", "serve", "--config"])
            .arg(&self.config)
            .current_dir(&*self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .env("MOOR_CRABBOX_BIN", &self.crabbox)
            .env(UPSTREAM_CA, &self.upstream.ca_path)
            .env(UPSTREAM, format!("127.0.0.1:{}", self.upstream.port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        fleet.spawn(&mut cmd).unwrap()
    }

    /// The Proxy row once it is ready with a lease, if it is.
    fn ready(&self) -> Option<Ready> {
        let db = rusqlite::Connection::open(self.root.join("state").join("moor.db")).ok()?;
        db.busy_timeout(Duration::from_secs(5)).ok()?;
        db.query_row(
            "SELECT box_port, pid, lease_owner, ca_cert FROM proxy \
             WHERE box = ?1 AND ready = 1 AND lease_owner IS NOT NULL",
            [self.box_id],
            |r| {
                Ok(Ready {
                    box_port: r.get(0)?,
                    pid: r.get(1)?,
                    owner: r.get(2)?,
                    ca_pem: r.get(3)?,
                })
            },
        )
        .ok()
    }

    /// Waits for a ready Proxy.
    fn wait_ready(&self) -> Ready {
        let start = Instant::now();
        loop {
            if let Some(ready) = self.ready() {
                return ready;
            }
            assert!(
                start.elapsed() < PROXY_UP,
                "no ready Proxy after {PROXY_UP:?}\n{}",
                self.logs()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Every log the controllers, the Proxy and the fake crabbox wrote.
    fn logs(&self) -> String {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&*self.root)
            .unwrap()
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "log"))
            .collect();
        paths.push(
            self.root
                .join("state/proxy")
                .join(format!("{}.log", self.box_id)),
        );
        paths.iter().fold(String::new(), |mut all, p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            let _ = writeln!(all, "== {}\n{text}", p.display());
            all
        })
    }

    /// The host sockets the fake crabbox tunnelled to, which a killed Proxy leaves behind.
    fn tunnelled_sockets(&self) -> Vec<PathBuf> {
        let log = std::fs::read_to_string(self.root.join("crabbox.log")).unwrap_or_default();
        log.lines()
            .filter_map(|l| l.split(" --reverse ").nth(1))
            .filter_map(|rest| rest.split_whitespace().next()?.split_once(':'))
            .map(|(_, socket)| PathBuf::from(socket))
            .collect()
    }
}

/// Removes the tunnelled sockets when dropped, after the fleet that made them is gone.
struct RemoveOnDrop<'a>(&'a Harness);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        for socket in self.0.tunnelled_sockets() {
            let _ = std::fs::remove_file(socket);
        }
    }
}

/// Sends requests until the loop has `more` successes beyond what it had.
fn run_for(traffic: &Traffic, more: usize, harness: &Harness) {
    let start = Instant::now();
    let want = traffic.tally().ok + more;
    while traffic.tally().ok < want {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the loop stalled: {:?}\n{}",
            traffic.tally(),
            harness.logs()
        );
        std::thread::sleep(PAUSE);
    }
}

#[test]
fn a_running_box_keeps_its_egress_while_the_controller_is_killed_and_restarted() {
    let harness = Harness::new();
    let _sockets = RemoveOnDrop(&harness);
    let mut fleet = Fleet::new();
    let first = harness.controller(&mut fleet, 1);
    let before = harness.wait_ready();
    let proxy = fleet.find("proxy serve").expect("a Proxy process");
    assert_eq!(proxy.pid.as_raw_nonzero().get(), before.pid);
    let agent = box_client(before.box_port, &before.ca_pem).unwrap();
    let url = format!("https://localhost:{}/ok", harness.upstream.port);
    let traffic = Traffic::start(PAUSE, move || {
        let got = agent.get(&url).call().map_err(|e| e.to_string())?;
        match got.status().as_u16() {
            200 => Ok(()),
            code => Err(format!("status {code}")),
        }
    });
    run_for(&traffic, 10, &harness);

    fleet.kill(first).unwrap();
    std::thread::sleep(GAP);
    run_for(&traffic, 10, &harness);
    harness.controller(&mut fleet, 2);
    // The new controller settles the Proxies one lease after it starts; keep going past that.
    std::thread::sleep(Duration::from_secs(35));
    run_for(&traffic, 10, &harness);

    let tally: Tally = traffic.stop();
    assert!(tally.failed.is_empty(), "{tally:?}\n{}", harness.logs());
    let after = harness.wait_ready();
    assert_eq!(after, before, "the Proxy was replaced");
    let seen = harness.upstream.seen();
    assert!(seen.len() >= tally.ok, "{} < {}", seen.len(), tally.ok);
    assert!(
        seen.iter().all(|a| a == &format!("Bearer {TOKEN}")),
        "{seen:?}"
    );
}
