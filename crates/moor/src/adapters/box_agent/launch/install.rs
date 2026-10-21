//! Reads the Box platform for the pinned gh.

use moor_core::boxscript::quote;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};

use super::{fail, Failure};

/// What a batch reports for one step.
pub(super) type StepResult = Result<StepDone, DriverError>;

/// The step that writes `uname -s -m` under the credential directory `dir` in the Box user's home
/// and prints it back; the home is not known yet, so the script reads it from `$HOME`. It exits
/// before creating anything when the Box has no readable process environment or no `$HOME`, the
/// cases where the launch fails at its home step and never cleans up. The directory and the file
/// are owner-only, as every other launch path makes them.
pub(super) fn platform_step(dir: &str) -> BoxStep {
    BoxStep::Run(format!(
        "[ -r /proc/self/environ ] && [ -n \"$HOME\" ] || exit 1; umask 077; \
         c=\"$HOME\"/{}; mkdir -p \"$c\" && uname -s -m > \"$c/uname\" && cat \"$c/uname\"",
        quote(dir)
    ))
}

/// The exit code of a finished `Run` step.
pub(super) fn check_outcome(result: Option<StepResult>) -> Result<i32, Failure> {
    match result {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => Ok(code),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => Err(fail("gh", &"the gh install was interrupted")),
        Some(Err(e)) => Err(fail("gh", &format!("the gh install failed: {e}"))),
        _ => Err(fail("gh", &"the gh install did not run")),
    }
}

/// The Box's operating system and architecture, from the result of the [`platform_step`].
pub(super) fn parse_platform(result: Option<StepResult>) -> Result<(String, String), Failure> {
    let stdout = match result {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => stdout,
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => {
            let message = format!("could not read the Box platform: exit {code}");
            return Err(fail("gh", &message));
        }
        other => return Err(check_outcome(other).err().unwrap_or_else(unreadable)),
    };
    let mut words = stdout.split_whitespace();
    match (words.next(), words.next()) {
        (Some(os), Some(arch)) => Ok((os.to_owned(), arch.to_owned())),
        _ => Err(unreadable()),
    }
}

/// The failure for a platform that cannot be read.
fn unreadable() -> Failure {
    fail("gh", &"could not read the Box platform")
}

#[cfg(test)]
mod tests;
