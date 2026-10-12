//! Copies a file or directory out of a Box through `crabbox cp`.

use std::path::Path;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;

use super::errors::{map_error, Op};
use super::{split_handle, static_env, target_args, Crabbox};

/// Time allowed for one copy.
const COPY_OUT_DEADLINE: Duration = Duration::from_mins(5);

/// Copies `from` in the Box behind `handle` to the host path `to`.
///
/// `crabbox cp` moves only the named path, so the worktree is never synced.
/// The command runs in `host_cwd`.
pub(super) fn copy_out(
    cb: &Crabbox,
    handle: &BoxHandle,
    from: &BoxPath,
    to: &Path,
    host_cwd: &Path,
) -> Result<(), DriverError> {
    let proc = Crabbox::spec(COPY_OUT_DEADLINE)
        .arg("cp")
        .args(target_args(handle))
        .arg(format!("SANDBOX:{}", from.as_str()))
        .arg(to)
        .cwd(host_cwd);
    let proc = static_env(proc, handle);
    cb.proc
        .run(&proc)
        .map(|_| ())
        .map_err(|e| map_error(Op::CopyOut, split_handle(handle).0, &e))
}

#[cfg(test)]
mod tests;
