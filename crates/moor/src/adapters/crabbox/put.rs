//! Writes one file in a Box over ssh, never through `crabbox run`.
//!
//! The content goes down ssh's stdin, so it never appears in an argument list.

use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Stdin, Tool};

use super::peek::ssh_target;
use super::{link, Crabbox};

/// Time allowed for the write.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);
/// Times a write is tried again after the connection to the Box was lost.
const LOST_RETRIES: u32 = 2;
/// Pause before each retry; the next call opens a fresh connection.
const LOST_PAUSE: Duration = Duration::from_secs(1);

/// Writes `bytes` to `path` in the Box behind `handle` and sets `mode`.
pub(super) fn put_file(
    cb: &Crabbox,
    handle: &BoxHandle,
    path: &BoxPath,
    bytes: &[u8],
    mode: u32,
) -> Result<(), DriverError> {
    let file = path.as_str();
    let dir = match file.rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => "/",
    };
    let dir = quote(dir);
    let file = quote(file);
    // umask keeps the file private while it is written; chmod then sets the real mode. The
    // write replaces the whole file, so running it again after a lost connection is safe.
    let remote = format!("umask 077 && mkdir -p {dir} && cat > {file} && chmod {mode:o} {file}");
    let mut retries = 0;
    loop {
        let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
        let write = ProcSpec::new(Tool::Ssh, Deadline(WRITE_DEADLINE))
            .args(words)
            .arg(remote.clone())
            .stdin(Stdin::Bytes(bytes.to_vec()));
        let err = match link::run(cb, handle, &write) {
            Ok(_) => return Ok(()),
            Err(ProcError::Timeout { after, .. }) => return Err(DriverError::Timeout(after)),
            Err(ProcError::Spawn(_)) => {
                return Err(DriverError::Unavailable(
                    "ssh could not be started".to_owned(),
                ))
            }
            Err(e) => e,
        };
        match link::lost(cb, handle, &err) {
            Some(lost) if retries >= LOST_RETRIES => return Err(lost),
            Some(_) => {
                retries += 1;
                cb.clock.sleep(LOST_PAUSE);
            }
            None => {
                return Err(DriverError::Failed {
                    summary: "box file write failed".to_owned(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
