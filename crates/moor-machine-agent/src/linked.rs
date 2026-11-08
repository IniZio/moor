//! The file `run` keeps in the state directory while it holds a link.
//!
//! It names the process that wrote it and when that process started, so a file left by a crash
//! or a reboot never reads as a link: it counts only while that same process still runs.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The file's name in the state directory.
pub const FILE: &str = "linked";

/// What the file holds.
#[derive(Serialize, Deserialize)]
struct Mark {
    /// The server linked to.
    server: String,
    /// The `run` process holding the link.
    pid: u32,
    /// When that process started, as [`start_of`] says.
    start: String,
}

/// When process `pid` started, unique across reboots and pid reuse; none when it is not running.
#[must_use]
pub fn start_of(pid: u32) -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        return (out.status.success() && !said.is_empty()).then_some(said);
    }
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, after_name) = stat.rsplit_once(')')?;
    let started = after_name.split_whitespace().nth(19)?;
    Some(format!("{} {started}", boot.trim()))
}

/// Writes the file at `path` for this process, linked to `server`.
///
/// # Errors
/// The file cannot be written.
pub fn write(path: &Path, server: &str) -> io::Result<()> {
    let pid = std::process::id();
    let mark = Mark {
        server: server.to_owned(),
        pid,
        start: start_of(pid).unwrap_or_default(),
    };
    fs::write(path, serde_json::to_vec(&mark).map_err(io::Error::other)?)
}

/// Whether the file at `path` was written by a process that still runs.
#[must_use]
pub fn live(path: &Path) -> bool {
    let Ok(text) = fs::read(path) else {
        return false;
    };
    let Ok(mark) = serde_json::from_slice::<Mark>(&text) else {
        return false;
    };
    !mark.start.is_empty() && start_of(mark.pid).is_some_and(|now| now == mark.start)
}
