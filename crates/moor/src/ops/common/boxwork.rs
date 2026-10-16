//! Finds out whether a Box holds work that exists nowhere else.

use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::boxscript::boxwork::{
    boxwork_script, is_full_sha, parse_boxwork, BOXWORK_FILE, BOXWORK_NO_CHECKOUT,
};
use moor_core::boxscript::quote;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath};

use super::provision::recorded_workdir;
use crate::cx::Cx;
use crate::store::{BoxRow, TaskRow};

/// Wall-clock allowance for the probe script.
pub(crate) const BOXWORK_DEADLINE: Duration = Duration::from_secs(60);

/// What a Box holds that the host does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoxWork {
    /// Every commit is on the host and the working tree is clean.
    Clean,
    /// Commits the host lacks, or files changed since the last commit.
    Unsaved {
        /// Commits in the Box that the host does not have.
        ahead: u32,
        /// Changed or untracked paths in the Box checkout.
        dirty: u32,
    },
    /// The driver no longer has the Box, so nothing in it can be saved.
    Gone,
    /// The probe could not tell; never read as clean.
    Unknown(String),
}

impl From<String> for BoxWork {
    fn from(reason: String) -> Self {
        Self::Unknown(reason)
    }
}

impl From<&str> for BoxWork {
    fn from(reason: &str) -> Self {
        Self::Unknown(reason.to_owned())
    }
}

/// A driver failure: [`BoxWork::Gone`] when the Box is missing, else unknown with `what`.
fn driver_fail(what: &str, e: &DriverError) -> BoxWork {
    match e {
        DriverError::NotFound => BoxWork::Gone,
        other => BoxWork::Unknown(format!("{what}: {other}")),
    }
}

/// What a probe sends to the Box: its two batch steps.
pub(crate) struct ProbePlan {
    /// The script step, then the step that reads its result file.
    pub(crate) steps: Vec<BoxStep>,
    /// The Box the steps run in.
    handle: BoxHandle,
    /// The host checkout the steps run from.
    worktree: PathBuf,
}

/// Runs the probe script in the Task's Box and reads its result back.
///
/// Anything that stops the probe from giving a definite answer is
/// [`BoxWork::Unknown`], so a caller never mistakes a failed probe for a clean Box.
pub(crate) fn probe(cx: &Cx, task: &TaskRow, boxed: &BoxRow) -> BoxWork {
    let plan = match plan(cx, task, boxed) {
        Ok(plan) => plan,
        Err(work) => return work,
    };
    let done = cx.driver().batch(
        &plan.handle,
        &[],
        &plan.steps,
        &plan.worktree,
        Deadline(BOXWORK_DEADLINE),
    );
    read(&plan, &mut done.into_iter())
}

/// Builds the probe for the Box of `boxed`; the `Err` is the answer when no probe can be sent.
pub(crate) fn plan(cx: &Cx, task: &TaskRow, boxed: &BoxRow) -> Result<ProbePlan, BoxWork> {
    let handle = boxed.handle.as_ref().ok_or("the box has no handle")?;
    let worktree = Path::new(task.worktree.as_deref().ok_or("the task has no worktree")?);
    let head = cx
        .git()
        .head(worktree)
        .map_err(|e| format!("cannot read the host HEAD: {e}"))?;
    if !is_full_sha(&head) {
        return Err("the host HEAD is not a full commit id".into());
    }
    // A base that names a branch is skipped: its commits are reachable from the host HEAD.
    let mut saved = vec![head.as_str()];
    saved.extend(task.base.as_deref().filter(|b| is_full_sha(b)));
    let script = boxwork_script(&saved).ok_or("cannot build the probe script")?;
    let workdir = match boxed.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => cx
            .driver()
            .workdir(handle, worktree)
            .map_err(|e| driver_fail("cannot find the box workdir", &e))?,
    };
    let path = BoxPath::under(workdir.as_str(), BOXWORK_FILE)
        .map_err(|e| format!("bad result path: {e}"))?;
    // A batch `Run` starts with no working directory, so the script enters the checkout itself.
    let script = format!("cd {} || exit 1\n{script}", quote(workdir.as_str()));
    Ok(ProbePlan {
        steps: vec![BoxStep::Run(script), BoxStep::Peek(path)],
        handle: handle.clone(),
        worktree: worktree.to_path_buf(),
    })
}

/// Reads the probe's verdict from the results of its steps.
///
/// Takes one item per step, fewer when the batch stopped early.
pub(crate) fn read(
    _plan: &ProbePlan,
    done: &mut dyn Iterator<Item = Result<StepDone, DriverError>>,
) -> BoxWork {
    match verdict(done) {
        Ok(work) | Err(work) => work,
    }
}

/// The verdict from the probe's step results, `Err` when no definite answer came back.
fn verdict(
    done: &mut dyn Iterator<Item = Result<StepDone, DriverError>>,
) -> Result<BoxWork, BoxWork> {
    match done.next() {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            ..
        })) => {}
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited {
                code: BOXWORK_NO_CHECKOUT,
            },
            ..
        })) => return Err("the Box has no git checkout".into()),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => return Err(format!("the probe exited {code}").into()),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => return Err("the probe was interrupted".into()),
        Some(Err(e)) => return Err(driver_fail("the probe could not run", &e)),
        Some(Ok(_)) | None => return Err("the probe left no result".into()),
    }
    let text = match done.next() {
        Some(Ok(StepDone::Peeked(Some(text)))) => text,
        Some(Err(e)) => return Err(driver_fail("cannot read the probe result", &e)),
        _ => return Err("the probe left no result".into()),
    };
    let counts = parse_boxwork(&text).ok_or("the probe result is unreadable")?;
    Ok(if counts.ahead == 0 && counts.dirty == 0 {
        BoxWork::Clean
    } else {
        BoxWork::Unsaved {
            ahead: counts.ahead,
            dirty: counts.dirty,
        }
    })
}

#[cfg(test)]
mod tests;
