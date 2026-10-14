//! Per-Task egress proxy and its request log.
//!
//! The log is the only file the proxy writes. It records the time, method,
//! route name and status of each request, and never the nonce, headers or
//! bodies.
//!
//! The nono module embeds nono-proxy as each Task's egress proxy.
//!
//! The supervisor owns the proxy's lifetime.

pub(crate) mod detached;
#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod head;
pub(crate) mod nono;
#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod relay;
#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod resolver;
#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod server;
pub(crate) mod supervisor;
#[cfg_attr(not(test), allow(dead_code, reason = "removed in the next change"))]
mod upstream;

pub(crate) use supervisor::{serve, Deps, Sink};

use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::seam::clock::Clock;
use moor_core::TaskId;

/// Append-only per-Task request log at `<state>/proxy/<task>.log`.
pub(crate) struct ProxyLog {
    /// Open log file; the lock keeps each line whole.
    file: Mutex<File>,
    /// Source of line timestamps.
    clock: Arc<dyn Clock>,
}

impl ProxyLog {
    /// Opens the log for `task`, creating `<state_dir>/proxy` (0700) and the
    /// file (0600) and tightening both modes when they already exist.
    pub(crate) fn open(
        state_dir: &Path,
        task: TaskId,
        clock: Arc<dyn Clock>,
    ) -> std::io::Result<Self> {
        let dir = state_dir.join("proxy");
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        std::fs::set_permissions(&dir, Permissions::from_mode(0o700))?;
        let path = dir.join(format!("{}.log", task.get()));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        std::fs::set_permissions(&path, Permissions::from_mode(0o600))?;
        Ok(Self {
            file: Mutex::new(file),
            clock,
        })
    }

    /// Appends one line `<ts_millis> <method> <route> <status>\n`.
    ///
    /// Write errors are swallowed: logging never fails a request.
    pub(crate) fn record(&self, method: &str, route: &str, status: u16) {
        let line = format!(
            "{} {} {} {}\n",
            self.clock.now().as_millis(),
            sanitize(method),
            sanitize(route),
            status
        );
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = file.write_all(line.as_bytes());
    }
}

/// Replaces spaces and non-printable or non-ASCII bytes with `_`, so one
/// request is always one line.
fn sanitize(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_graphic() {
                char::from(b)
            } else {
                '_'
            }
        })
        .collect()
}
