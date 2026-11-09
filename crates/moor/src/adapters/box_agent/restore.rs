//! Puts the agent's stored session back on a Box that lacks it, before a resume.

use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::AgentAdapter;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, LaunchReq};
use moor_core::seam::proc::Deadline;

/// Where the archive is put, relative to the Box workdir.
const ARCHIVE: &str = ".moor/restore.tgz";

/// Time allowed for the check and for unpacking the archive.
const DEADLINE: Duration = Duration::from_mins(1);

/// Whether the Box of `req` holds session `id`.
pub(super) fn holds(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    req: &LaunchReq,
    id: &str,
) -> Result<bool, EngineError> {
    match run(driver, req, &profile.session_check_script(plain(id)?)) {
        Ok(0) => Ok(true),
        Ok(1) => Ok(false),
        Ok(code) => Err(fail("check", format!("exit {code}"))),
        Err(e) => Err(fail("check", e)),
    }
}

/// Puts `archive` on the Box of `req` and unpacks it there.
pub(super) fn restore(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    req: &LaunchReq,
    id: &str,
    archive: &[u8],
) -> Result<(), EngineError> {
    let script = profile.restore_script(plain(id)?, ARCHIVE);
    let at = BoxPath::under(req.workdir.as_str(), ARCHIVE).map_err(|e| fail("put", e))?;
    driver
        .put_file(&req.handle, &at, archive, 0o600)
        .map_err(|e| fail("put", e))?;
    match run(driver, req, &script) {
        Ok(0) => Ok(()),
        Ok(code) => Err(fail("unpack", format!("exit {code}"))),
        Err(e) => Err(fail("unpack", e)),
    }
}

/// `id` when it is safe to name a file by: letters, digits, `-` and `_`.
fn plain(id: &str) -> Result<&str, EngineError> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(id)
        .ok_or_else(|| fail("check", format!("session id {id:?} is not a plain name")))
}

/// The exit code of `script` run in the workdir of `req`.
fn run(driver: &dyn Driver, req: &LaunchReq, script: &str) -> Result<i32, String> {
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
        env: Vec::new(),
        cwd: Some(req.workdir.clone()),
        output: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(DEADLINE)) {
        Ok(ExecOutcome::Exited { code }) => Ok(code),
        Ok(ExecOutcome::Interrupted) => Err("interrupted".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// A failed `step` of the restore.
fn fail(step: &str, why: impl std::fmt::Display) -> EngineError {
    EngineError::Transcript(format!("restore {step}: {why}"))
}
