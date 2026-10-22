//! The ssh command line an agent pane runs to reach a Box.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;

use super::peek::ssh_target;
use super::{pin, Crabbox};

/// `ssh -t <target>` for the Box behind `handle`, refused when the target does not pin the
/// Box's host key. The lookup takes no lease lock.
pub(super) fn connect_argv(cb: &Crabbox, handle: &BoxHandle) -> Result<Vec<String>, DriverError> {
    let target = ssh_target(cb, handle)?;
    pin::check(&target)?;
    let mut argv = vec!["ssh".to_owned(), "-t".to_owned()];
    argv.extend(target);
    Ok(argv)
}

#[cfg(test)]
mod tests;
