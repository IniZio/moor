//! Archives the agent's transcript in the Box and copies it to the host.

use std::path::Path;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;

/// Where the archive is built, relative to the Box workdir.
const ARCHIVE: &str = ".moor/transcripts.tgz";

/// Wall-clock allowance for building the archive.
const ARCHIVE_DEADLINE: Duration = Duration::from_mins(2);

/// Builds the archive on the Box, then copies it to `dest`.
pub(super) fn save(
    driver: &dyn Driver,
    profile: &dyn AgentProfile,
    s: &SessionRef,
    dest: &Path,
) -> Result<(), EngineError> {
    let fail = |what: &str, why: String| EngineError::Transcript(format!("{what}: {why}"));
    let spec = ExecSpec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            profile.transcript_script(ARCHIVE),
        ],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(s.worktree.clone()),
        class: None,
    };
    match driver.exec(&s.handle, &spec, Deadline(ARCHIVE_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        Ok(ExecOutcome::Exited { code }) => return Err(fail("archive", format!("exit {code}"))),
        Ok(ExecOutcome::Interrupted) => return Err(fail("archive", "interrupted".to_owned())),
        Err(e) => return Err(fail("archive", e.to_string())),
    }
    let from =
        BoxPath::under(s.workdir.as_str(), ARCHIVE).map_err(|e| fail("copy", e.to_string()))?;
    driver
        .copy_out(&s.handle, &from, dest, &s.worktree)
        .map_err(|e| fail("copy", e.to_string()))
}

#[cfg(test)]
mod tests;
