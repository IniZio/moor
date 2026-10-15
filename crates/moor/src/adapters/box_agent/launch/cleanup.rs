//! Cleanup of a failed launch.

use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::LaunchReq;
use moor_core::seam::proc::Deadline;

/// Time allowed for removing the credential directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);

/// Removes the credential and temp directories on the Box.
pub(super) fn remove_cred(
    driver: &dyn Driver,
    req: &LaunchReq,
    (cred, tmp): (&BoxPath, &BoxPath),
) -> Result<(), String> {
    let argv = ["rm", "-rf", cred.as_str(), tmp.as_str()].map(str::to_owned);
    exec(driver, req, argv.to_vec())
}

/// Runs the shell `line` on the Box, for removing what a failed launch made.
pub(super) fn run_line(driver: &dyn Driver, req: &LaunchReq, line: &str) -> Result<(), String> {
    let argv = ["sh", "-c", line].map(str::to_owned);
    exec(driver, req, argv.to_vec())
}

/// Runs `argv` on the Box.
fn exec(driver: &dyn Driver, req: &LaunchReq, argv: Vec<String>) -> Result<(), String> {
    let spec = ExecSpec {
        argv,
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(req.worktree.clone()),
        class: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(CLEANUP_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(format!("cleanup exited {code}")),
        Ok(ExecOutcome::Interrupted) => Err("cleanup was interrupted".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}
