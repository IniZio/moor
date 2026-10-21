//! Opening the reverse tunnel after freeing its Box port from a stale forward.
//!
//! When a tunnel is dropped, its ssh and `ProxyCommand` are killed without closing the
//! connection, so the Box's `sshd-session` can keep the half-open session, and with it the
//! `-R` listener port, until it times out. Every new `ssh -R` would then fail to bind.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use moor_core::seam::driver::{DriverError, ExecSpec, TunnelHandle};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId};

use super::run::say;
use super::{Deps, TICK};

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

/// How often the renewal thread checks whether a heartbeat is due.
const POLL: Duration = Duration::from_millis(20);

/// Opens a tunnel serving the front `socket` on `box_port` in the Box.
///
/// The lease `owner` holds on `box_id` is renewed every [`TICK`] while this runs: the port clear
/// alone can outlast the lease, and a lapsed lease lets a concurrent `moor run` take the row over.
///
/// Unless `fresh` (no tunnel was ever opened into this Box, or it just cold-booted, so no stale
/// listener can exist), the
/// port is freed first. A failed or timed-out cleanup is reported and the open goes ahead.
pub(super) fn open(
    deps: &Deps<'_>,
    (box_id, owner): (BoxId, &str),
    handle: &BoxHandle,
    (box_port, socket): (u16, &Path),
    worktree: &Path,
    fresh: bool,
) -> Result<Box<dyn TunnelHandle>, DriverError> {
    renewing(deps, box_id, owner, || {
        if !fresh {
            clear(deps, handle, box_port, worktree);
        }
        deps.driver
            .reverse_tunnel(handle, box_port, socket, worktree)
    })
}

/// Runs `work` while a scoped thread heartbeats the lease every [`TICK`].
///
/// A failed or refused heartbeat is not reported here: the caller beats again when `work` returns
/// and acts on the outcome.
fn renewing<T>(deps: &Deps<'_>, box_id: BoxId, owner: &str, work: impl FnOnce() -> T) -> T {
    let (done, gone) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut last = deps.clock.instant();
            // Disconnect is the signal to stop; nothing is ever sent.
            while gone.recv_timeout(POLL) == Err(mpsc::RecvTimeoutError::Timeout) {
                if deps.clock.instant().saturating_duration_since(last) >= TICK {
                    last = deps.clock.instant();
                    drop(deps.store.heartbeat(box_id, owner));
                }
            }
        });
        let out = work();
        drop(done);
        out
    })
}

/// Ends any `sshd-session` holding `box_port` in the Box, reporting the outcome.
fn clear(deps: &Deps<'_>, handle: &BoxHandle, box_port: u16, worktree: &Path) {
    let spec = ExecSpec {
        argv: clear_argv(box_port),
        env: Vec::new(),
        cwd: None,
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
