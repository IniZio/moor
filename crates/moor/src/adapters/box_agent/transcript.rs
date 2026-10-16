//! Archives the agent's transcript in the Box and copies it to the host.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::{AgentProfile, NOTHING_TO_ARCHIVE};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, StepDone};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;

/// Where the archive is built, relative to the Box workdir.
const ARCHIVE: &str = ".moor/transcripts.tgz";

/// Wall-clock allowance for building the archive.
const ARCHIVE_DEADLINE: Duration = Duration::from_secs(15);

/// Builds the archive on the Box, then copies it to `dest`.
pub(super) fn save(
    driver: &dyn Driver,
    profile: &dyn AgentProfile,
    s: &SessionRef,
    dest: &Path,
) -> Result<(), EngineError> {
    let fail = |what: &str, why: String| EngineError::Transcript(format!("{what}: {why}"));
    let script = format!(
        "cd {} && {}",
        quote(s.workdir.as_str()),
        profile.transcript_script(ARCHIVE)
    );
    let connect = driver
        .connect_argv(&s.handle, &s.worktree)
        .map_err(|e| fail("archive", e.to_string()))?;
    let steps = [BoxStep::Run(script)];
    let done = driver
        .batch(
            &s.handle,
            &connect,
            &steps,
            &s.worktree,
            Deadline(ARCHIVE_DEADLINE),
        )
        .into_iter()
        .next();
    match done {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => {
            if stdout.trim() == NOTHING_TO_ARCHIVE {
                return Err(EngineError::NothingToArchive);
            }
        }
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => return Err(fail("archive", format!("exit {code}"))),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => return Err(fail("archive", "interrupted".to_owned())),
        Some(Ok(_)) => return Err(fail("archive", "unexpected result".to_owned())),
        Some(Err(e)) => return Err(fail("archive", e.to_string())),
        None => return Err(fail("archive", "no result".to_owned())),
    }
    let from =
        BoxPath::under(s.workdir.as_str(), ARCHIVE).map_err(|e| fail("copy", e.to_string()))?;
    driver
        .copy_out(&s.handle, &from, dest, &s.worktree)
        .map_err(|e| fail("copy", e.to_string()))
}

#[cfg(test)]
mod tests;
