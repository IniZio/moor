//! The host-key check every Box ssh call must keep.
//!
//! moor runs ssh to a Box with the words `crabbox ssh` printed for its lease. Those words must
//! keep host key checking on, so moor refuses to use them when they do not; the rules are
//! [`moor_core::ssh::pin`]'s, over this host's own files.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::time::Duration;

use rustix::fs::{access, Access};

use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};
use moor_core::ssh::pin::{self, KnownHosts};

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
    pin::check(&out.stdout, &Local).map_err(|why| refused(&why))
}

/// The known-hosts files on this host.
struct Local;

impl KnownHosts for Local {
    /// A device such as `/dev/null` is caught by its type, however its path is spelled or linked.
    fn is_device(&self, file: &str) -> bool {
        fs::metadata(file).is_ok_and(|m| m.file_type().is_char_device())
    }

    /// Checks permissions only and writes nothing.
    fn can_add_to(&self, file: &str) -> bool {
        let path = std::path::Path::new(file);
        let writable = |p: &std::path::Path| access(p, Access::WRITE_OK).is_ok();
        if path.exists() {
            return writable(path);
        }
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        writable(dir.unwrap_or_else(|| std::path::Path::new(".")))
    }
}

/// The refusal of words that do not pin the host key, saying `why`.
fn refused(why: &str) -> DriverError {
    DriverError::Failed {
        summary: pin::refusal(why),
    }
}

#[cfg(test)]
mod tests;
