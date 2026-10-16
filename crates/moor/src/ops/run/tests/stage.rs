//! Helpers for scripting and reading the Box execs `run` issues around the user's command.

use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

use super::Fx;
use crate::testing::driver::FakeDriver;

/// A driver whose `results` are the command's: the staging `mkdir` before it succeeds.
pub(in crate::ops::run) fn command_results(
    results: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
) -> FakeDriver {
    let mkdir = Ok(ExecOutcome::Exited { code: 0 });
    FakeDriver::new().exec_results(std::iter::once(mkdir).chain(results))
}

/// The Box execs that ran the user's command, not the staging around it.
pub(in crate::ops::run) fn command_execs(f: &Fx) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
    let mut all = f.driver.execs();
    all.retain(|(_, s, _)| s.argv.first().is_some_and(|a| a == "sh"));
    all
}
