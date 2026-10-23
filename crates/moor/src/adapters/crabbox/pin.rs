//! The host-key check every Box ssh call must keep.
//!
//! moor runs ssh to a Box with the words `crabbox ssh` printed for its lease. Those words must
//! keep host key checking on, so moor refuses to use them when they do not. ssh itself says what
//! the words come to: `ssh -G` prints the options it would connect with, after reading every
//! flag and config file, and opens no connection. `StrictHostKeyChecking` must be yes or
//! accept-new, which records a new key and rejects a changed one. It must not be no, off or
//! false. No known-hosts file may be `/dev/null`, another device, `none` or empty.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::time::Duration;

use rustix::fs::{access, Access};

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
    if !matches!(strict, Some("true" | "yes" | "accept-new")) {
        return Err(refused("they turn host key checking off"));
    }
    let files = match files {
        Some(files) if !files.split(' ').any(holds_no_keys) => files,
        _ => {
            return Err(refused(
                "they name /dev/null or no file for UserKnownHostsFile",
            ))
        }
    };
    let first = files.split(' ').next().unwrap_or_default();
    if strict == Some("accept-new") && !can_add_to(first) {
        return Err(refused(&format!(
            "accept-new cannot record a new key in {first}, which is not writable"
        )));
    }
    Ok(())
}

/// Whether ssh can add a host key to the known-hosts file `file`: the file is writable, or it is
/// missing and its directory is writable. Checks permissions only and writes nothing.
fn can_add_to(file: &str) -> bool {
    let path = std::path::Path::new(file);
    let writable = |p: &std::path::Path| access(p, Access::WRITE_OK).is_ok();
    if path.exists() {
        return writable(path);
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    writable(dir.unwrap_or_else(|| std::path::Path::new(".")))
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
