//! Runs the repo `setup:` lines inside a freshly acquired Box.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::SetupOutcome;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxPath, Warning};

use crate::cx::Cx;

/// Wall-clock allowance for one setup line.
pub(crate) const SETUP_LINE_DEADLINE: Duration = Duration::from_mins(30);

/// How the setup lines ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupRun {
    /// The repo declares no setup lines.
    Skipped,
    /// Every line exited 0.
    Ok,
    /// A line failed; later lines did not run.
    Failed {
        /// The failing line.
        line: String,
        /// The script's exit code, when it exited.
        exit: Option<i32>,
    },
}

impl SetupRun {
    /// The outcome recorded on the Task.
    pub(crate) fn outcome(&self) -> SetupOutcome {
        match self {
            Self::Skipped | Self::Ok => SetupOutcome::Ok,
            Self::Failed { line, .. } => SetupOutcome::Failed { line: line.clone() },
        }
    }

    /// The outcome shown to the user.
    pub(crate) fn view(&self) -> SetupView {
        match self {
            Self::Skipped => SetupView::default(),
            Self::Ok => SetupView {
                ran: true,
                ok: true,
                line: None,
            },
            Self::Failed { line, .. } => SetupView {
                ran: true,
                ok: false,
                line: Some(line.clone()),
            },
        }
    }

    /// The exit code of a failed script.
    pub(crate) const fn exit(&self) -> Option<i32> {
        match self {
            Self::Failed { exit, .. } => *exit,
            _ => None,
        }
    }

    /// Whether a line failed.
    pub(crate) const fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// Wall-clock allowance for a setup script of `lines` lines: one line allowance per line.
pub(crate) fn setup_deadline(lines: usize) -> Duration {
    let n = u32::try_from(lines).unwrap_or(u32::MAX);
    SETUP_LINE_DEADLINE.saturating_mul(n)
}

/// Runs the setup lines as one synced script in `workdir`, stopping at the first failure.
///
/// Sync is one-way, so after a failure the script's log is copied back from the Box; the
/// failing line comes from there.
pub(crate) fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    worktree: &Path,
    cfg: &TaskSettings,
) -> SetupRun {
    if cfg.setup.is_empty() {
        return SetupRun::Skipped;
    }
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), setup_script(&cfg.setup)],
        env: cfg
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        cwd: Some(workdir.clone()),
        sync: SyncMode::Full,
        output: None,
        host_cwd: Some(worktree.to_path_buf()),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let deadline = Deadline(setup_deadline(cfg.setup.len()));
    let (reason, exit) = match cx.driver().exec(handle, &spec, deadline) {
        Ok(ExecOutcome::Exited { code: 0 }) => return SetupRun::Ok,
        Ok(ExecOutcome::Exited { code }) => (format!("exit {code}"), Some(code)),
        Ok(ExecOutcome::Interrupted) => ("interrupted".to_owned(), None),
        Err(e) => (e.to_string(), None),
    };
    let log = fetch_log(cx, handle, workdir, worktree);
    let line = cfg
        .setup
        .get(failed_step(&log, cfg.setup.len()))
        .cloned()
        .unwrap_or_default();
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: format!("setup line `{line}` failed: {reason}"),
    });
    SetupRun::Failed { line, exit }
}

/// Copies the script's log out of the Box and reads it; empty when either step fails.
fn fetch_log(cx: &Cx, handle: &BoxHandle, workdir: &BoxPath, worktree: &Path) -> String {
    let to = worktree.join(".moor").join("setup.log");
    let Ok(from) = BoxPath::under(workdir.as_str(), ".moor/setup.log") else {
        return String::new();
    };
    let made = to
        .parent()
        .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok());
    if !made || cx.driver().copy_out(handle, &from, &to, worktree).is_err() {
        return String::new();
    }
    std::fs::read_to_string(to).unwrap_or_default()
}

/// Index of the failing step: the last one the log announced, else the first.
fn failed_step(log: &str, steps: usize) -> usize {
    let announced = log
        .lines()
        .filter(|l| l.starts_with("moor setup: $ "))
        .count();
    announced.clamp(1, steps.max(1)) - 1
}

#[cfg(test)]
mod tests;
