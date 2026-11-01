//! What the `moor-notify` tests share: the front as a child process, a scripted controller
//! and a webhook receiver.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) mod controller;
pub(crate) mod receiver;

/// The HMAC key every test hook signs with.
pub(crate) const KEY: &str = "test-key";

/// Writes a hooks file for `email` posting `events` (the defaults when none) to `url`, with
/// its key file beside it, and returns the config path.
pub(crate) fn write_config(dir: &Path, email: &str, url: &str, events: Option<&str>) -> PathBuf {
    let key = dir.join("key");
    std::fs::write(&key, format!("{KEY}\n")).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let events = events
        .map(|e| format!("    events: {e}\n"))
        .unwrap_or_default();
    let config = dir.join("notify.yaml");
    std::fs::write(
        &config,
        format!(
            "hooks:\n  - email: {email}\n    url: {url}\n    secret_file: {}\n{events}",
            key.display()
        ),
    )
    .unwrap();
    config
}

/// A running `moor-notify`, killed on drop, with every stderr line it wrote.
pub(crate) struct Front {
    /// The process.
    child: Child,
    /// Its stderr lines so far.
    lines: Arc<Mutex<Vec<String>>>,
}

impl Front {
    /// Runs the front on `socket` with `config` and state under `state`, with no proxy in its
    /// environment so it reaches the loopback receiver directly.
    pub(crate) fn start(socket: &Path, config: &Path, state: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_moor-notify"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg("--socket")
            .arg(socket)
            .arg("--config")
            .arg(config)
            .arg("--state-dir")
            .arg(state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().unwrap();
        let sink = Arc::clone(&lines);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        });
        Self { child, lines }
    }

    /// Waits until a log line contains `text` and returns it; fails the test after 30 s.
    pub(crate) fn wait_for_line(&self, text: &str) -> String {
        for _ in 0..1500 {
            if let Some(line) = self.lines().into_iter().find(|l| l.contains(text)) {
                return line;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("no log line with {text:?} in {:#?}", self.lines());
    }

    /// Every log line so far.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for Front {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
