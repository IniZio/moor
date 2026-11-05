//! Runs the repo `setup:` lines inside a freshly acquired Box.

use std::fmt::Write as _;
use std::fs::{DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use moor_core::boxscript::detached::attached_script;
use moor_core::boxscript::quote;
use moor_core::boxscript::setup::setup_script;
use moor_core::config::BoxSettings;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::decision::ExtraEvent;
use moor_core::exec_env::{prelude_stop, Purpose};
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::view::SetupView;
use moor_core::{BoxHandle, BoxId, BoxPath, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};

use lines::follow_lines;

/// How many trailing log lines a failure warning carries.
const LOG_TAIL_LINES: usize = 20;

/// Longest tail line kept, in characters; progress bars can emit huge ones.
const LOG_LINE_CHARS: usize = 300;

/// Why a setup that a forced close interrupted failed.
pub(crate) const FORCED_CLOSE: &str = "setup interrupted by a forced close";

/// The host file in the log dir that receives the setup exec's own output, which holds the
/// prelude's reason when it stops the exec before the script runs.
const OUTPUT_FILE: &str = "setup-exec.log";

/// Where the setup script writes its log, relative to the Box workdir.
const LOG_REL: &str = ".moor/setup.log";

/// Attaches after a cut channel before the setup is given up; the run in the Box goes on.
const REFOLLOWS: u32 = 5;

/// Pause before attaching again after a cut.
const REFOLLOW_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

/// Seconds a stopped setup run gets between TERM and KILL.
const CANCEL_GRACE_SECS: u32 = 5;

/// The id of the detached run that holds one setup script in the Box: the same for every
/// attach to one start's script, so a restarted controller follows the run it left. `attempt`
/// names the start, `workdir` the tree and `place` the line of a script run line by line.
pub(crate) fn run_id(attempt: i64, workdir: &BoxPath, place: Option<(u32, u32)>) -> i64 {
    use sha2::{Digest as _, Sha256};
    let key = format!("{attempt}\n{}\n{place:?}", workdir.as_str());
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(digest.get(..8).unwrap_or(&[0; 8]));
    // Negative, so it never names a detached command run, whose ids count up from 1.
    -i64::try_from(u64::from_le_bytes(bytes) >> 2).unwrap_or(0) - 1
}

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
/// The script runs detached in the Box as run `run`, so it outlives the channel that follows
/// it: a cut channel attaches again, and a controller that restarts attaches to the same run.
///
/// The script's log is copied back from the Box to `setup.log` in `log_dir`; its tail and,
/// after a failure, the failing line come from there, or from the exec's own output when the
/// copy fails. The script gets the environment the agent gets in
/// the Box, including the Proxy variables when `proxy` is set.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, its provider and run, workdir, log dir, settings, Proxy and progress sink"
)]
pub(crate) fn run_setup(
    cx: &Cx,
    handle: &BoxHandle,
    (provider, run): (&str, i64),
    workdir: &BoxPath,
    log_dir: &Path,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    on_line: &(dyn Fn(ExtraEvent) + Sync),
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
    let output = log_dir.join(OUTPUT_FILE);
    let result = attach(cx, &site, cfg, proxy, &output, run, on_line);
    let elapsed = cx.clock().instant().saturating_duration_since(started);
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    if let Some(stop) = read_stop(&output) {
        cx.warn(Warning {
            code: "setup_failed".to_owned(),
            message: format!("setup did not run: {stop}"),
        });
        return SetupRun::Failed {
            line: stop,
            exit: exit_code(&result),
            ms,
            tail: String::new(),
        };
    }
    let mut log = fetch_log(cx, handle, workdir, log_dir);
    if log.is_empty() {
        log = String::from_utf8_lossy(&std::fs::read(&output).unwrap_or_default()).into_owned();
    }
    let tail = tail_lines(&log).join("\n");
    let (reason, exit) = match result {
        Ok(ExecOutcome::Exited { code: 0 }) => return SetupRun::Ok { ms, tail },
        Ok(ExecOutcome::Exited { code }) => (format!("exit {code}"), Some(code)),
        Ok(ExecOutcome::Interrupted) => ("interrupted".to_owned(), None),
        Err(Failure::Prepare(e)) => (e, None),
        Err(Failure::Exec(e)) => (e.to_string(), None),
    };
    let line = if cx.env().cancel().is_cancelled() {
        FORCED_CLOSE.to_owned()
    } else {
        cfg.setup
            .get(failed_step(&log, cfg.setup.len()))
            .cloned()
            .unwrap_or_default()
    };
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

/// The exit code `result` carries, when the script exited.
const fn exit_code(result: &Result<ExecOutcome, Failure>) -> Option<i32> {
    match result {
        Ok(ExecOutcome::Exited { code }) => Some(*code),
        _ => None,
    }
}

/// Where the prelude stopped the exec before any setup line ran, read from the exec's `output`.
fn read_stop(output: &Path) -> Option<String> {
    let text = String::from_utf8_lossy(&std::fs::read(output).ok()?).into_owned();
    prelude_stop(&text).map(str::to_owned)
}

/// Builds the setup environment, then attaches to run `run`, which launches the script once,
/// until the run ends, its output into `output`. A cut channel attaches again and reads the
/// log from its start, so only lines not reported yet reach `on_line`. A run given up on while
/// it may still be going is stopped in the Box.
fn attach(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    output: &Path,
    run: i64,
    on_line: &(dyn Fn(ExtraEvent) + Sync),
) -> Result<ExecOutcome, Failure> {
    let purpose = Purpose::Setup {
        lines: cfg.setup.len(),
    };
    let exec = box_exec::prepare(cx, site, proxy, cfg, purpose).map_err(Failure::Prepare)?;
    if let Some(dir) = output.parent() {
        private_dir(dir).map_err(|e| Failure::Prepare(format!("{}: {e}", dir.display())))?;
    }
    let script = ["sh", "-c", &setup_script(&cfg.setup)].map(quote).join(" ");
    let spec = ExecSpec {
        argv: exec.argv(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            attached_script(run, &script),
        ]),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: Some(Output::Log(output.to_path_buf())),
    };
    let Deadline(limit) = exec.deadline();
    let started = cx.clock().instant();
    let reported = Reported::default();
    let deliver = |e: ExtraEvent| {
        if reported.first(&e) {
            on_line(e);
        }
    };
    let mut cuts = 0;
    loop {
        // A log left by an earlier attach would announce lines this one has not reached.
        let _ = std::fs::remove_file(output);
        let left = limit.saturating_sub(cx.clock().instant().saturating_duration_since(started));
        let result = follow_lines(cx, cfg, output, &deliver, || {
            cx.driver()
                .exec(site.handle, &spec, Deadline(left))
                .map_err(Failure::Exec)
        });
        let ended = matches!(result, Ok(ExecOutcome::Exited { .. }) | Err(Failure::Prepare(_)));
        let more = lines::cut(&result) && cuts < REFOLLOWS && !left.is_zero();
        if ended || !more || cx.env().cancel().is_cancelled() {
            if !ended {
                let _ = cx.driver().cancel_run(site.handle, run, CANCEL_GRACE_SECS);
            }
            return result;
        }
        cuts += 1;
        cx.clock().sleep(REFOLLOW_AFTER);
    }
}

/// The highest line each kind of line event has reported, so an attach that reads the log from
/// its start again reports only what is new.
#[derive(Default)]
struct Reported {
    /// Highest line reported started.
    started: AtomicU32,
    /// Highest line reported finished.
    finished: AtomicU32,
}

impl Reported {
    /// Whether `event` is reported for the first time; records it.
    fn first(&self, event: &ExtraEvent) -> bool {
        let (seen, index) = match event {
            ExtraEvent::SetupLineStarted { index, .. } => (&self.started, *index),
            ExtraEvent::SetupLineFinished { index, .. } => (&self.finished, *index),
            _ => return true,
        };
        seen.fetch_max(index, Ordering::SeqCst) < index
    }
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

mod lines;

#[cfg(test)]
mod tests;
