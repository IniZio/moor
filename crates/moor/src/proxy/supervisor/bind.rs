//! Opening the reverse tunnel after freeing its Box port from a stale forward.
//!
//! When a tunnel is dropped, its ssh and `ProxyCommand` are killed without closing the
//! connection, so the Box's `sshd-session` can keep the half-open session, and with it the
//! `-R` listener port, until it times out. Every new `ssh -R` would then fail to bind.

use std::path::Path;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{DriverError, ExecSpec, TunnelHandle};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

use super::run::say;
use super::Deps;

/// Time allowed for the Box-side cleanup; a `crabbox run` on the external provider costs about 20 s
/// before the script starts.
const CLEAR_DEADLINE: Duration = Duration::from_secs(45);

/// Shell run in the Box with the port as `$1`.
///
/// `ss` lists the listeners on exactly that port with their owners; only the pids named
/// `sshd-session` are signalled, and nothing happens when there are none.
const CLEAR_SCRIPT: &str = r#"for pid in $(ss -Htlnp "sport = :$1" 2>/dev/null | grep -o '"sshd-session",pid=[0-9]*' | cut -d= -f2); do kill "$pid" 2>/dev/null; done; exit 0"#;

/// The Box command that ends any `sshd-session` listening on `box_port`.
pub(super) fn clear_argv(box_port: u16) -> Vec<String> {
    ["sh", "-c", CLEAR_SCRIPT, "sh"]
        .into_iter()
        .map(str::to_owned)
        .chain([box_port.to_string()])
        .collect()
}

/// Opens a tunnel serving `host_port` on `box_port` in the Box.
///
/// Unless `fresh` (no tunnel was ever opened into this Box, or it just cold-booted, so no stale
/// listener can exist), the
/// port is freed first. A failed or timed-out cleanup is reported and the open goes ahead.
pub(super) fn open(
    deps: &Deps<'_>,
    handle: &BoxHandle,
    box_port: u16,
    host_port: u16,
    worktree: &Path,
    fresh: bool,
) -> Result<Box<dyn TunnelHandle>, DriverError> {
    if !fresh {
        clear(deps, handle, box_port, worktree);
    }
    deps.driver
        .reverse_tunnel(handle, box_port, host_port, worktree)
}

/// Ends any `sshd-session` holding `box_port` in the Box, reporting the outcome.
fn clear(deps: &Deps<'_>, handle: &BoxHandle, box_port: u16, worktree: &Path) {
    let spec = ExecSpec {
        argv: clear_argv(box_port),
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(worktree.to_path_buf()),
        class: None,
    };
    let outcome = match deps.driver.exec(handle, &spec, Deadline(CLEAR_DEADLINE)) {
        Ok(done) => format!("freed port {box_port} in the box: {done:?}"),
        Err(err) => format!("cannot free port {box_port} in the box: {err}"),
    };
    say(&deps.stderr, format_args!("{outcome}"));
}

#[cfg(test)]
mod tests;
