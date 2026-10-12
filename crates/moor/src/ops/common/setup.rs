//! Runs the repo `setup:` lines inside a freshly acquired Box.

use std::time::Duration;

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
    },
}

impl SetupRun {
    /// The outcome recorded on the Task.
    pub(crate) fn outcome(&self) -> SetupOutcome {
        match self {
            Self::Skipped | Self::Ok => SetupOutcome::Ok,
            Self::Failed { line } => SetupOutcome::Failed { line: line.clone() },
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
            Self::Failed { line } => SetupView {
                ran: true,
                ok: false,
                line: Some(line.clone()),
            },
        }
    }

    /// Whether a line failed.
    pub(crate) const fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// Runs each setup line in `workdir`, stopping at the first failure.
pub(crate) fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    cfg: &TaskSettings,
) -> SetupRun {
    if cfg.setup.is_empty() {
        return SetupRun::Skipped;
    }
    let env: Vec<(String, String)> = cfg
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for line in &cfg.setup {
        let spec = ExecSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), line.clone()],
            env: env.clone(),
            cwd: Some(workdir.clone()),
            sync: SyncMode::None,
            output: None,
            host_cwd: None,
            class: None,
        };
        let reason = match cx
            .driver()
            .exec(handle, &spec, Deadline(SETUP_LINE_DEADLINE))
        {
            Ok(ExecOutcome::Exited { code: 0 }) => continue,
            Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
            Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
            Err(e) => e.to_string(),
        };
        cx.warn(Warning {
            code: "setup_failed".to_owned(),
            message: format!("setup line `{line}` failed: {reason}"),
        });
        return SetupRun::Failed { line: line.clone() };
    }
    SetupRun::Ok
}

#[cfg(test)]
mod tests;
