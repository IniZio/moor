//! The forwarder line runs under a real `sh` against a fake Box whose fake forwarder opens a
//! loopback port when it starts.

use std::path::{Path, PathBuf};

use moor_testkit::FakeEgress;
use rustix::process::{kill_process, Pid, Signal};

use super::super::egress::{forwarder, EGRESS_FAILED, FORWARDER};

/// Tools the line calls that the fake Box takes from the host.
const HOST_TOOLS: &[&str] = &[
    "sh", "grep", "sed", "sleep", "cat", "tail", "mv", "flock", "setsid",
];

/// A forwarder that records its start, opens the fake port and waits for TERM.
const LISTENS: &str = "#!/bin/sh\necho \"forward $*\" >>\"$HOME/fake.log\"\necho start >>\"$HOME/fake-starts\"\ntrap 'kill $! 2>/dev/null; exit 0' TERM\nsleep 30 & wait $!\n";

/// A forwarder that starts but never listens.
const DEAF: &str = "#!/bin/sh\necho 'cannot listen' >&2\nsleep 30\n";

struct FakeBox {
    home: PathBuf,
    bin: PathBuf,
    port: FakeEgress,
    _dir: tempfile::TempDir,
}

impl FakeBox {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("moor-forward-")
            .tempdir()
            .unwrap();
        let home = dir.path().to_path_buf();
        let bin = home.join("bin");
        std::fs::create_dir_all(home.join(".moor")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        for tool in HOST_TOOLS {
            let found = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists())
                .unwrap();
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        let port = FakeEgress::start(&home, &bin).unwrap();
        Self {
            home,
            bin,
            port,
            _dir: dir,
        }
    }

    fn path(&self, suffix: &str) -> PathBuf {
        self.home.join(format!("{FORWARDER}{suffix}"))
    }

    /// Delivers `body` as the forwarder, as `.new` when `replace`, with a config on `port`.
    fn deliver(&self, body: &str, port: u16, replace: bool) {
        let to = self.path(if replace { ".new" } else { "" });
        moor_testkit::write_executable(&to, body).unwrap();
        std::fs::write(
            self.path(".json"),
            format!("{{\"listen\":\"127.0.0.1:{port}\",\"upstream\":\"ws://u/egress\",\"token\":\"t\"}}"),
        )
        .unwrap();
    }

    fn run(&self) -> (bool, String) {
        let script = format!("{}\necho ran\n", forwarder(self.port.port()));
        let out = std::process::Command::new(self.bin.join("sh"))
            .args(["-c", &script])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .output()
            .unwrap();
        (
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("ran"),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn starts(&self) -> usize {
        std::fs::read_to_string(self.home.join("fake.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("forward "))
            .count()
    }
}

impl Drop for FakeBox {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(self.path(".pid"))
            .ok()
            .and_then(|p| p.trim().parse::<i32>().ok())
            .filter(|p| *p > 0)
            .and_then(Pid::from_raw)
        {
            let _ = kill_process(pid, Signal::KILL);
        }
    }
}

#[test]
fn without_a_config_nothing_starts() {
    let b = FakeBox::new();
    let (ok, err) = b.run();
    assert!(ok, "{err}");
    assert_eq!(b.starts(), 0);
}

#[test]
fn a_delivered_forwarder_starts_once_on_its_port() {
    let b = FakeBox::new();
    b.deliver(LISTENS, b.port.port(), false);
    let (ok, err) = b.run();
    assert!(ok, "{err}");
    let (ok, err) = b.run();
    assert!(ok, "{err}");
    assert_eq!(b.starts(), 1, "a running forwarder is kept");
    let log = std::fs::read_to_string(b.home.join("fake.log")).unwrap();
    assert!(
        log.contains(&format!("--config {}", b.path(".json").display())),
        "{log}"
    );
}

#[test]
fn a_config_for_another_port_starts_nothing() {
    let b = FakeBox::new();
    b.deliver(LISTENS, b.port.port().wrapping_add(1), false);
    let (ok, err) = b.run();
    assert!(ok, "{err}");
    assert_eq!(b.starts(), 0);
}

#[test]
fn a_new_forwarder_replaces_the_running_one() {
    let b = FakeBox::new();
    b.deliver(LISTENS, b.port.port(), false);
    assert!(b.run().0);
    let replacement = format!("{LISTENS}# replacement\n");
    b.deliver(&replacement, b.port.port(), true);
    let old = std::fs::read_to_string(b.path(".pid")).unwrap();
    b.port.close();
    let (ok, err) = b.run();
    assert!(ok, "{err}");
    assert_eq!(b.starts(), 2);
    assert!(!b.path(".new").exists(), "the replacement was moved in");
    assert_eq!(std::fs::read_to_string(b.path("")).unwrap(), replacement);
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", old.trim())).unwrap_or_default();
    assert!(
        stat.is_empty() || stat.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')),
        "the old forwarder was stopped: {stat}"
    );
}

#[test]
fn a_forwarder_that_never_listens_stops_the_exec() {
    let b = FakeBox::new();
    b.deliver(DEAF, b.port.port(), false);
    let (ok, err) = b.run();
    assert!(!ok);
    assert!(
        err.contains(&format!(
            "moor: {EGRESS_FAILED}: the egress forwarder is not listening on 127.0.0.1:{}: cannot listen",
            b.port.port()
        )),
        "{err}"
    );
}
