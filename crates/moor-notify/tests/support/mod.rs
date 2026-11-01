//! What the `moor-notify` tests share: the front as a child process, a scripted controller
//! and a webhook receiver.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) mod controller;
pub(crate) mod receiver;

/// A running `moor-notify`, killed on drop, with every stderr line it wrote.
pub(crate) struct Front {
    /// The process.
    child: Child,
    /// Its stderr lines so far.
    lines: Arc<Mutex<Vec<String>>>,
}

impl Front {
    /// Runs the front on `socket`, polling every 20 ms, with no proxy in its environment and
    /// `--allow-private` so it reaches the loopback receiver directly.
    pub(crate) fn start(socket: &Path) -> Self {
        Self::spawn(socket, &["--allow-private"])
    }

    /// Runs the front on `socket` with its address guard on, as in production.
    pub(crate) fn guarded(socket: &Path) -> Self {
        Self::spawn(socket, &[])
    }

    /// Runs the front on `socket` with `extra` arguments.
    fn spawn(socket: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_moor-notify"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg("--socket")
            .arg(socket)
            .args(["--poll-ms", "20"])
            .args(extra)
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

    /// Whether the process is still running.
    pub(crate) fn is_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// Waits for the process to exit and returns how; fails the test after 30 s.
    pub(crate) fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        for _ in 0..1500 {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the front never exited");
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
