//! Runs the repo `setup:` lines inside a freshly acquired Box.

use std::fmt::Write as _;
use std::fs::{DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use moor_core::boxscript::setup::setup_script;
use moor_core::config::BoxSettings;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxId, BoxPath, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};

/// How many trailing log lines a failure warning carries.
const LOG_TAIL_LINES: usize = 20;

/// Longest tail line kept, in characters; progress bars can emit huge ones.
const LOG_LINE_CHARS: usize = 300;

/// Where the setup script writes its log, relative to the Box workdir.
const LOG_REL: &str = ".moor/setup.log";

/// The host directory that keeps Box `id`'s copy of its setup log: `<state>/logs/<id>`.
pub(crate) fn log_dir(state: &Path, id: BoxId) -> PathBuf {
    state.join("logs").join(id.get().to_string())
}

/// How the setup lines ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupRun {
    /// The repo declares no setup lines.
    Skipped,
    /// Every line exited 0.
    Ok {
        /// Time the script took, in milliseconds.
        ms: u64,
        /// The last lines of the script's log, tidied; empty when the log was unreadable.
        tail: String,
    },
    /// A line failed; later lines did not run.
    Failed {
        /// The failing line.
        line: String,
        /// The script's exit code, when it exited.
        exit: Option<i32>,
        /// Time the script took, in milliseconds.
        ms: u64,
        /// The last lines of the script's log, tidied; empty when the log was unreadable.
        tail: String,
    },
}

impl SetupRun {
    /// The outcome recorded on the Box.
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
            Self::Ok { ms, tail } => SetupView {
                ran: true,
                ok: true,
                line: None,
                ms: Some(*ms),
                tail: Some(tail.clone()).filter(|t| !t.is_empty()),
            },
            Self::Failed { line, ms, tail, .. } => SetupView {
                ran: true,
                ok: false,
                line: Some(line.clone()),
                ms: Some(*ms),
                tail: Some(tail.clone()).filter(|t| !t.is_empty()),
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
            Self::Ok { ms, .. } | Self::Failed { ms, .. } => Some(*ms),
        }
    }

    /// Whether a line failed.
    pub(crate) const fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// Runs the setup lines as one script in `workdir`, stopping at the first failure.
///
/// The script's log is copied back from the Box to `setup.log` in `log_dir`; its tail and,
/// after a failure, the failing line come from there. The script gets the environment the agent gets in
/// the Box, including the Proxy variables when `proxy` is set.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, its provider and workdir, log dir, settings and Proxy"
)]
pub(crate) fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    provider: &str,
    workdir: &BoxPath,
    log_dir: &Path,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
) -> SetupRun {
    if cfg.setup.is_empty() {
        return SetupRun::Skipped;
    }
    let site = Site {
        handle,
        provider,
        workdir: Some(workdir),
    };
    let started = cx.clock().instant();
    let result = exec_script(cx, &site, cfg, proxy);
    let elapsed = cx.clock().instant().saturating_duration_since(started);
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let log = fetch_log(cx, handle, workdir, log_dir);
    let tail = tail_lines(&log).join("\n");
    let (reason, exit) = match result {
        Ok(ExecOutcome::Exited { code: 0 }) => return SetupRun::Ok { ms, tail },
        Ok(ExecOutcome::Exited { code }) => (format!("exit {code}"), Some(code)),
        Ok(ExecOutcome::Interrupted) => ("interrupted".to_owned(), None),
        Err(Failure::Prepare(e)) => (e, None),
        Err(Failure::Exec(e)) => (e.to_string(), None),
    };
    let line = cfg
        .setup
        .get(failed_step(&log, cfg.setup.len()))
        .cloned()
        .unwrap_or_default();
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: failure_message(&line, &reason, workdir, &log),
    });
    SetupRun::Failed {
        line,
        exit,
        ms,
        tail,
    }
}

/// Builds the setup environment and runs the script once.
fn exec_script(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
) -> Result<ExecOutcome, Failure> {
    let purpose = Purpose::Setup {
        lines: cfg.setup.len(),
    };
    let exec = box_exec::prepare(cx, site, proxy, cfg, purpose).map_err(Failure::Prepare)?;
    let spec = ExecSpec {
        argv: exec.argv(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            setup_script(&cfg.setup),
        ]),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: None,
    };
    cx.driver()
        .exec(site.handle, &spec, exec.deadline())
        .map_err(Failure::Exec)
}

/// A setup that failed because the Proxy would not start; no line ran, so none is blamed.
///
/// `name` names the Box in the by-hand retry hint.
pub(crate) fn proxy_failed(cx: &Cx, name: &str, reason: &str) -> SetupRun {
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: format!(
            "setup did not run: the Proxy could not start: {reason}; once it starts, run each \
             `setup:` line by hand with `moor run {name} -- '<line>'`"
        ),
    });
    SetupRun::Failed {
        line: "the Proxy could not start".to_owned(),
        exit: None,
        ms: 0,
        tail: String::new(),
    }
}

/// Why the script did not run to an exit.
enum Failure {
    /// The environment could not be built.
    Prepare(String),
    /// The Box exec failed.
    Exec(DriverError),
}

/// The warning text: the failing line, where the full log lives in the Box, and its tail.
fn failure_message(line: &str, reason: &str, workdir: &BoxPath, log: &str) -> String {
    let path = format!("{}/{LOG_REL}", workdir.as_str());
    let mut msg = format!("setup line `{line}` failed: {reason}\nfull log in the Box: {path}");
    let tail = tail_lines(log);
    if tail.is_empty() {
        return msg;
    }
    let _ = write!(msg, "\nlast {} lines:\n{}", tail.len(), tail.join("\n"));
    msg
}

/// The last lines of `log`, each tidied.
fn tail_lines(log: &str) -> Vec<String> {
    // A terminal shows only the last frame of a line redrawn with `\r`.
    let lines: Vec<&str> = log
        .lines()
        .filter_map(|l| l.rsplit('\r').find(|seg| !seg.is_empty()))
        .collect();
    lines
        .iter()
        .skip(lines.len().saturating_sub(LOG_TAIL_LINES))
        .map(|l| tidy(l))
        .collect()
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

/// Copies the script's log out of the Box into the owner-only `log_dir` and reads it; empty
/// when either step fails.
fn fetch_log(cx: &Cx, handle: &BoxHandle, workdir: &BoxPath, log_dir: &Path) -> String {
    let to = log_dir.join("setup.log");
    let Ok(from) = BoxPath::under(workdir.as_str(), LOG_REL) else {
        return String::new();
    };
    if private_dir(log_dir).is_err() || cx.driver().copy_out(handle, &from, &to).is_err() {
        discard(&to);
        return String::new();
    }
    let is_file = std::fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_file());
    if !is_file || std::fs::set_permissions(&to, Permissions::from_mode(0o600)).is_err() {
        discard(&to);
        return String::new();
    }
    String::from_utf8_lossy(&std::fs::read(&to).unwrap_or_default()).into_owned()
}

/// Makes `dir` readable by the owner only; it and any missing parent are created that way.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, Permissions::from_mode(0o700))
}

/// Removes whatever sits at `path`, without following a symlink there.
fn discard(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(path);
        }
        Err(_) => {}
    }
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
