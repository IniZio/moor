//! The host-key check every Box ssh call must keep.
//!
//! moor runs ssh to a Box with the words `crabbox ssh` printed for its lease. Those words are
//! what pins the Box's host key, so moor refuses to use them when they do not. ssh itself says
//! what the words come to: `ssh -G` prints the options it would connect with, after reading
//! every flag and config file, and opens no connection. `StrictHostKeyChecking` must be yes, and
//! no known-hosts file may be `/dev/null`, another device, `none` or empty.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::time::Duration;

use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::Crabbox;

/// Time allowed for `ssh -G`, which reads only local files.
const READ_DEADLINE: Duration = Duration::from_secs(5);

/// Ok when `words`, the ssh arguments for a Box, pin its host key.
pub(super) fn check(cb: &Crabbox, words: &[String]) -> Result<(), DriverError> {
    let spec = ProcSpec::new(Tool::Ssh, Deadline(READ_DEADLINE))
        .arg("-G")
        .args(words.iter().cloned());
    let out = cb.options.run(&spec).map_err(|e| match e {
        ProcError::Spawn(_) => DriverError::Unavailable("ssh could not be started".to_owned()),
        _ => refused("ssh -G could not read them"),
    })?;
    let mut strict = None;
    let mut files = None;
    for line in out.stdout.lines() {
        match line.split_once(' ') {
            Some(("stricthostkeychecking", v)) => strict = Some(v),
            Some(("userknownhostsfile", v)) => files = Some(v),
            _ => {}
        }
    }
    if !matches!(strict, Some("true" | "yes")) {
        return Err(refused("they leave StrictHostKeyChecking short of yes"));
    }
    match files {
        Some(files) if !files.split(' ').any(holds_no_keys) => Ok(()),
        _ => Err(refused(
            "they name /dev/null or no file for UserKnownHostsFile",
        )),
    }
}

/// Whether the known-hosts file ssh printed as `file` can hold no host key. A device such as
/// `/dev/null` is caught by its type, however its path is spelled or linked.
fn holds_no_keys(file: &str) -> bool {
    file.is_empty()
        || file == "none"
        || fs::metadata(file).is_ok_and(|m| m.file_type().is_char_device())
}

/// The refusal of words that do not pin the host key, saying `why`.
fn refused(why: &str) -> DriverError {
    DriverError::Failed {
        summary: format!(
            "moor refuses the ssh options crabbox printed for this Box: {why}, so the Box's host \
             key would not be checked"
        ),
    }
}

#[cfg(test)]
mod tests;
