//! The extra steps of `run --record`: read the Box HEAD, refuse a dirty Box checkout, wrap the
//! command so the Box runs it from a clean checkout of HEAD.

use std::time::Duration;

use moor_core::boxscript::command_line;
use moor_core::boxscript::quote;
use moor_core::boxscript::record::{head_script, parse_head, record_script, HEAD_FILE};
use moor_core::seam::driver::{BoxStep, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxPath, ErrorKind, Sha};

use super::ready::Target;
use crate::cx::Cx;
use crate::error::CommandError;

/// Wall-clock allowance for reading the Box HEAD.
const HEAD_DEADLINE: Duration = Duration::from_secs(60);

/// Reads the Box checkout's HEAD in one batch and returns it as the sha the run is stored under.
///
/// The record script cleans untracked files before the command, so a checkout with uncommitted
/// changes is refused rather than losing them and storing a sha that is not what ran.
///
/// # Errors
/// [`CommandError`] when the checkout has uncommitted changes or its HEAD cannot be read.
pub(super) fn checkout(cx: &Cx, target: &Target) -> Result<Sha, CommandError> {
    let failed = |why: String| {
        CommandError::new(
            ErrorKind::Failed,
            format!("cannot read the Box HEAD: {why}"),
        )
    };
    let workdir = workdir(cx, target).map_err(failed)?;
    let path = BoxPath::under(workdir.as_str(), HEAD_FILE).map_err(|e| failed(e.to_string()))?;
    let script = format!(
        "cd {} || exit 1\n{}",
        quote(workdir.as_str()),
        head_script()
    );
    let steps = [BoxStep::Run(script), BoxStep::Peek(path)];
    let done = cx
        .driver()
        .batch(&target.handle, &[], &steps, Deadline(HEAD_DEADLINE));
    let text = match done.as_slice() {
        [Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            ..
        }), Ok(StepDone::Peeked(Some(text)))] => text,
        [Ok(StepDone::Ran { outcome, .. })] => {
            return Err(failed(format!("the probe ended {outcome:?}")));
        }
        [.., Err(e)] => return Err(failed(e.to_string())),
        _ => return Err(failed("the probe left no result".to_owned())),
    };
    let (head, dirty) = parse_head(text).ok_or_else(|| failed("unreadable result".to_owned()))?;
    if dirty > 0 {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "the Box checkout has uncommitted changes; commit them first",
        ));
    }
    Sha::parse(&head).map_err(|e| failed(e.to_string()))
}

/// The argv a record run executes: the repo setup lines, then the command, in one script that
/// fails the run when HEAD is not `head` or a tracked file changed afterwards.
pub(super) fn wrap(setup: &[String], argv: &[String], head: &Sha) -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        record_script(setup, &command_line(argv), head.as_str()),
    ]
}

/// The Box workdir, as stored or else as the driver names it.
pub(super) fn workdir(cx: &Cx, target: &Target) -> Result<BoxPath, String> {
    target.workdir.as_ref().map_or_else(
        || {
            cx.driver()
                .workdir(&target.handle)
                .map_err(|e| e.to_string())
        },
        |w| Ok(w.clone()),
    )
}

#[cfg(test)]
mod tests;
