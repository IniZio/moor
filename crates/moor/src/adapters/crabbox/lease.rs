//! The private host dir every crabbox call on one lease runs from.
//!
//! crabbox claims a lease for the git repo its process runs in, or for its cwd outside any
//! repo, and refuses a later call from another root. Each lease gets its own dir under the
//! state dir, kept for the lease's whole life, so no caller's checkout becomes a claim root.
//! `GIT_CEILING_DIRECTORIES` stops git, and crabbox's own walk, at the dir that holds them, so
//! a state dir inside a git repo cannot leak that repo into the claim.

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::ProcSpec;

use super::{split_handle, static_env, target_args, Crabbox};

/// The env var git and crabbox read to stop walking up to a repo.
pub(super) const CEILING: &str = "GIT_CEILING_DIRECTORIES";

impl Crabbox {
    /// `<state>/leases/<slug>`, the dir every crabbox call on `handle` runs from.
    ///
    /// # Errors
    /// [`DriverError::Failed`] when moor keeps no state dir or the handle's name is not a
    /// plain file name.
    pub(super) fn lease_dir(&self, handle: &BoxHandle) -> Result<PathBuf, DriverError> {
        let root = self.leases.as_ref().ok_or_else(|| DriverError::Failed {
            summary: "moor has no state dir to run crabbox from".to_owned(),
        })?;
        let name = split_handle(handle).1;
        let plain = !name.starts_with('.')
            && !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !plain {
            return Err(DriverError::Failed {
                summary: format!("box handle {:?} cannot name a lease dir", handle.as_str()),
            });
        }
        Ok(root.join(name))
    }

    /// `spec` run from the lease dir of `handle`, made 0700 when missing, with the git
    /// ceiling at its parent and the static provider's env.
    ///
    /// # Errors
    /// [`DriverError::Failed`] when the dir cannot be named or made.
    pub(super) fn at_lease(
        &self,
        spec: ProcSpec,
        handle: &BoxHandle,
    ) -> Result<ProcSpec, DriverError> {
        let dir = self.lease_dir(handle)?;
        let made = DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .and_then(|()| fs::canonicalize(&dir));
        let dir = made.map_err(|e| DriverError::Failed {
            summary: format!("cannot make the lease dir {}: {e}", dir.display()),
        })?;
        let ceiling = dir.parent().unwrap_or(&dir).to_owned();
        let spec = spec.cwd(dir).env(CEILING, ceiling.to_string_lossy());
        Ok(static_env(spec, handle))
    }

    /// Removes the lease dir of `handle` once its lease is gone; a dir that cannot be
    /// removed only stays behind.
    pub(super) fn drop_lease_dir(&self, handle: &BoxHandle) {
        if let Ok(dir) = self.lease_dir(handle) {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// The command a person runs to release `handle` by hand.
    pub(super) fn stop_by_hand(&self, handle: &BoxHandle) -> Option<String> {
        let dir = self.lease_dir(handle).ok()?;
        Some(format!(
            "run `crabbox stop {} --delete` from {}",
            target_args(handle).join(" "),
            dir.display()
        ))
    }
}

#[cfg(test)]
mod tests;
