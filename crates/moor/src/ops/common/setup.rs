//! Runs the repo `setup:` lines inside a freshly acquired Box.

use std::fmt::Write as _;
use std::path::Path;

use moor_core::boxscript::setup::setup_script;
use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::SetupOutcome;
use moor_core::exec_env::{Purpose, SETUP_LINE_DEADLINE};
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxPath, TaskId, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};

/// How many trailing log lines a failure warning carries.
const LOG_TAIL_LINES: usize = 20;

/// Longest tail line kept, in characters; progress bars can emit huge ones.
const LOG_LINE_CHARS: usize = 300;

/// Where the setup script writes its log, relative to the Box workdir.
const LOG_REL: &str = ".moor/setup.log";

/// How the setup lines ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupRun {
    /// The repo declares no setup lines.
    Skipped,
    /// Every line exited 0.
    Ok {
        /// Time the script took, in milliseconds.
        ms: u64,
    },
    /// A line failed; later lines did not run.
    Failed {
        /// The failing line.
        line: String,
        /// The script's exit code, when it exited.
        exit: Option<i32>,
        /// Time the script took, in milliseconds.
        ms: u64,
    },
}

impl SetupRun {
    /// The outcome recorded on the Task.
    pub(crate) fn outcome(&self) -> SetupOutcome {
        match self {
            Self::Skipped | Self::Ok { .. } => SetupOutcome::Ok,
            Self::Failed { line, .. } => SetupOutcome::Failed { line: line.clone() },
        }
    }

    /// The outcome shown to the user.
    pub(crate) fn view(&self) -> SetupView {
        match self {
            Self::Skipped => SetupView::default(),
            Self::Ok { ms } => SetupView {
                ran: true,
                ok: true,
                line: None,
                ms: Some(*ms),
            },
            Self::Failed { line, ms, .. } => SetupView {
                ran: true,
                ok: false,
                line: Some(line.clone()),
                ms: Some(*ms),
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

    /// How long the script took, in milliseconds; none when no setup ran.
    pub(crate) const fn ms(&self) -> Option<u64> {
        match self {
            Self::Skipped => None,
            Self::Ok { ms } | Self::Failed { ms, .. } => Some(*ms),
        }
    }

    /// Whether a line failed.
    pub(crate) const fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// Runs the setup lines as one script in `workdir` after syncing the worktree as `sync` says,
/// stopping at the first failure.
///
/// Sync is one-way, so after a failure the script's log is copied back from the Box; the
/// failing line comes from there. The script gets the environment the agent gets in the Box,
/// including the Proxy variables when `proxy` is set. `task` is the Task the Box belongs to, so
/// the script gets that Task's environment whether or not a Proxy started.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, its provider and workdir, worktree, settings, sync mode, Proxy and Task"
)]
pub(crate) fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    provider: &str,
    workdir: &BoxPath,
    worktree: &Path,
    cfg: &TaskSettings,
    sync: SyncMode,
    proxy: Option<&BoxProxy>,
    task: TaskId,
) -> SetupRun {
    if cfg.setup.is_empty() {
        return SetupRun::Skipped;
    }
    let site = Site {
        handle,
        provider,
        workdir: Some(workdir),
        worktree,
        task: Some(task),
    };
    let purpose = Purpose::Setup {
        lines: cfg.setup.len(),
    };
    let started = cx.clock().instant();
    let mut sandboxed = false;
    let result = box_exec::prepare(cx, &site, proxy, cfg, purpose)
        .map_err(Failure::Prepare)
        .and_then(|exec| {
            sandboxed = exec.is_sandboxed();
            let spec = ExecSpec {
                argv: exec.argv(vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    setup_script(&cfg.setup),
                ]),
                env: exec.vars(),
                cwd: exec.cwd(),
                sync,
                output: None,
                host_cwd: Some(worktree.to_path_buf()),
                class: Some(cfg.size.value.as_str().to_owned()),
            };
            let out = cx.driver().exec(handle, &spec, exec.deadline());
            if matches!(out, Err(_) | Ok(ExecOutcome::Interrupted)) {
                exec.cleanup(cx, &site);
            }
            out.map_err(Failure::Exec)
        });
    let elapsed = cx.clock().instant().saturating_duration_since(started);
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let (reason, exit) = match result {
        Ok(ExecOutcome::Exited { code: 0 }) => return SetupRun::Ok { ms },
        Ok(ExecOutcome::Exited { code }) => (format!("exit {code}"), Some(code)),
        Ok(ExecOutcome::Interrupted) => ("interrupted".to_owned(), None),
        Err(Failure::Prepare(e)) => (e, None),
        Err(Failure::Exec(e)) => (e.to_string(), None),
    };
    let log = fetch_box_file(cx, handle, workdir, worktree, LOG_REL);
    let sandbox = denials::fetch(cx, handle, workdir, worktree, sandboxed);
    let line = cfg
        .setup
        .get(failed_step(&log, cfg.setup.len()))
        .cloned()
        .unwrap_or_default();
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: failure_message(&line, &reason, workdir, &log, &sandbox),
    });
    SetupRun::Failed { line, exit, ms }
}

/// Why the script did not run to an exit.
enum Failure {
    /// The environment could not be built.
    Prepare(String),
    /// The Box exec failed.
    Exec(DriverError),
}

/// Syncs the worktree into the Box with an exec that only checks the workdir is a git checkout, so
/// a failure here is the sync failing or leaving no checkout, and never a setup line.
///
/// # Errors
/// The reason, worded for the user, when the sync did not finish.
pub(crate) fn sync_checkout(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    worktree: &Path,
    cfg: &TaskSettings,
) -> Result<(), String> {
    let spec = ExecSpec {
        argv: ["git", "rev-parse", "--git-dir"]
            .map(str::to_owned)
            .to_vec(),
        env: Vec::new(),
        cwd: Some(workdir.clone()),
        sync: SyncMode::Full,
        output: None,
        host_cwd: Some(worktree.to_path_buf()),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let reason = match cx
        .driver()
        .exec(handle, &spec, Deadline(SETUP_LINE_DEADLINE))
    {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!(
            "exit {code}; the sync failed or left no git checkout (with submodules, \
             push HEAD to origin first)"
        ),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    Err(format!(
        "syncing the checkout into the Box failed: {reason}"
    ))
}

/// The warning text: the failing line, where the full log lives in the Box, and its tail.
///
/// `sandbox` is the sandbox's own account of what it denied, inserted after the first line.
fn failure_message(
    line: &str,
    reason: &str,
    workdir: &BoxPath,
    log: &str,
    sandbox: &str,
) -> String {
    let path = format!("{}/{LOG_REL}", workdir.as_str());
    let mut msg =
        format!("setup line `{line}` failed: {reason}{sandbox}\nfull log in the Box: {path}");
    // A terminal shows only the last frame of a line redrawn with `\r`.
    let lines: Vec<&str> = log
        .lines()
        .filter_map(|l| l.rsplit('\r').find(|seg| !seg.is_empty()))
        .collect();
    if lines.is_empty() {
        return msg;
    }
    let tail: Vec<String> = lines
        .iter()
        .skip(lines.len().saturating_sub(LOG_TAIL_LINES))
        .map(|l| tidy(l))
        .collect();
    let _ = write!(msg, "\nlast {} lines:\n{}", tail.len(), tail.join("\n"));
    msg
}

/// Drops ANSI escape sequences and other control characters, and caps the length.
fn tidy(line: &str) -> String {
    let mut clean = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\u{1b}', Some('[')) => {
                chars.next();
                while chars
                    .next_if(|p| ('\u{20}'..='\u{3f}').contains(p))
                    .is_some()
                {}
                chars.next_if(|p| ('\u{40}'..='\u{7e}').contains(p));
            }
            ('\u{1b}', Some(']')) => {
                chars.next();
                while let Some(o) = chars.next() {
                    if o == '\u{7}' || (o == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ if c == '\t' || !c.is_control() => clean.push(c),
            _ => {}
        }
    }
    if clean.chars().count() <= LOG_LINE_CHARS {
        return clean;
    }
    let mut cut: String = clean.chars().take(LOG_LINE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// Copies a file under the Box workdir out to the same place under the worktree and reads it;
/// empty when either step fails.
fn fetch_box_file(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    worktree: &Path,
    rel: &str,
) -> String {
    let to = worktree.join(rel);
    let Ok(from) = BoxPath::under(workdir.as_str(), rel) else {
        return String::new();
    };
    let made = to
        .parent()
        .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok());
    if !made || cx.driver().copy_out(handle, &from, &to, worktree).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&std::fs::read(to).unwrap_or_default()).into_owned()
}

/// Index of the failing step: the last one the log announced, else the first.
fn failed_step(log: &str, steps: usize) -> usize {
    let announced = log
        .lines()
        .filter(|l| l.starts_with("moor setup: $ "))
        .count();
    announced.clamp(1, steps.max(1)) - 1
}

mod denials;

#[cfg(test)]
mod tests;
