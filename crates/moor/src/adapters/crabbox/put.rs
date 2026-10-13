//! Writes one file in a Box over ssh, never through `crabbox run`.
//!
//! The content goes down ssh's stdin, so it never appears in an argument list.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Stdin, Tool};

use super::peek::ssh_target;
use super::Crabbox;

/// Time allowed for the write.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);

/// Writes `bytes` to `path` in the Box behind `handle` and sets `mode`.
/// The target lookup runs in `host_cwd`, so the lease claim matches that repo.
pub(super) fn put_file(
    cb: &Crabbox,
    handle: &BoxHandle,
    path: &BoxPath,
    bytes: &[u8],
    mode: u32,
    host_cwd: &Path,
) -> Result<(), DriverError> {
    let words = ssh_target(cb, handle, host_cwd)?;
    let file = path.as_str();
    let dir = match file.rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => "/",
    };
    let dir = quote(dir);
    let file = quote(file);
    // umask keeps the file private while it is written; chmod then sets the real mode.
    let remote = format!("umask 077 && mkdir -p {dir} && cat > {file} && chmod {mode:o} {file}");
    let write = ProcSpec::new(Tool::Ssh, Deadline(WRITE_DEADLINE))
        .args(words)
        .arg(remote)
        .stdin(Stdin::Bytes(bytes.to_vec()));
    match cb.proc.run(&write) {
        Ok(_) => Ok(()),
        Err(ProcError::Timeout { after, .. }) => Err(DriverError::Timeout(after)),
        Err(ProcError::Spawn(_)) => Err(DriverError::Unavailable(
            "ssh could not be started".to_owned(),
        )),
        Err(_) => Err(DriverError::Failed {
            summary: "box file write failed".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests;
