//! Finds out whether a Box holds work that exists nowhere else.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::boxwork::{
    boxwork_script, is_full_sha, parse_boxwork, BOXWORK_FILE, BOXWORK_NO_CHECKOUT,
};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::BoxPath;

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

/// Runs the probe script in the Task's Box and reads its result back.
///
/// Anything that stops the probe from giving a definite answer is
/// [`BoxWork::Unknown`], so a caller never mistakes a failed probe for a clean Box.
pub(crate) fn probe(cx: &Cx, task: &TaskRow, boxed: &BoxRow) -> BoxWork {
    match run(cx, task, boxed) {
        Ok(work) | Err(work) => work,
    }
}

/// Probes the Box, naming the reason when no definite answer comes back.
fn run(cx: &Cx, task: &TaskRow, boxed: &BoxRow) -> Result<BoxWork, BoxWork> {
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
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: Some(workdir.clone()),
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(worktree.to_path_buf()),
        class: None,
    };
    match cx.driver().exec(handle, &spec, Deadline(BOXWORK_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        Ok(ExecOutcome::Exited {
            code: BOXWORK_NO_CHECKOUT,
        }) => return Err("the Box has no git checkout".into()),
        Ok(ExecOutcome::Exited { code }) => return Err(format!("the probe exited {code}").into()),
        Ok(ExecOutcome::Interrupted) => return Err("the probe was interrupted".into()),
        Err(e) => return Err(driver_fail("the probe could not run", &e)),
    }
    let path = BoxPath::under(workdir.as_str(), BOXWORK_FILE)
        .map_err(|e| format!("bad result path: {e}"))?;
    let text = cx
        .driver()
        .peek_file(handle, &path, worktree)
        .map_err(|e| driver_fail("cannot read the probe result", &e))?
        .ok_or("the probe left no result")?;
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
