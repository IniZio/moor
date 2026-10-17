//! One Box release for every way a Task gives up its Box: close, failed start, cancel, remove.
//!
//! Each step runs on its own: a failed step is a warning and every later step still runs. Each
//! lease row is written once per release, with every failed step that answers on it. The
//! lease rows are all kept or all dropped, and only a driver that released the Box drops them.
//! A Box outside the Mac layout on a host that goes with it has nothing left once the driver
//! deleted it, or found it gone, so the rows go even after a failed step. On a host that
//! outlives the release (a Mac layout, or a static host whose lease only drops the claim) they go
//! only when no step failed and verify was clean; otherwise every row stays for the next
//! release. Caps that cannot be read count as a static host. A driver that failed to release the
//! Box fails every row.

use std::path::Path;
use std::time::Duration;

use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, Released, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::task_key::release_sim::{failed_lines, verify_report};
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxHandle, Warning};

use self::layout::layout_from_leases;
use self::record::{record_attempts, Outcome};
use self::steps::{plan, script, MacTarget, Step};
use super::provision::recorded_workdir;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::close::teardown::teardown;
use crate::ops::show::redact_nonce;
use crate::store::lease::LeaseRow;
use crate::store::{Store, TaskRow};

pub(crate) mod steps;

/// Longest summary kept for a failed step.
const SUMMARY_MAX: usize = 300;

/// First marker line of a release script's output.
const BEGIN: &str = "MOOR_RELEASE_BEGIN";

/// Last marker line of a release script's output.
const END: &str = "MOOR_RELEASE_END";

/// Wall-clock limit of a release script that has no limit of its own.
const STEP_DEADLINE: Duration = Duration::from_secs(30);

/// Limit for killing a process tree and for deleting simulator devices: a Box can hold hundreds
/// of simulator processes and devices, and the fixed 30 seconds ran out with 277 of them.
const SWEEP_DEADLINE: Duration = Duration::from_secs(120);

/// Time stop-agent gets beyond its wait: the kill and the shell round trip.
const STOP_AGENT_SLACK: Duration = Duration::from_secs(30);

/// Lines of output kept when a script's end marker never came.
const TAIL_LINES: usize = 5;

/// Characters of that output kept.
const TAIL_CHARS: usize = 400;

/// What one Box release did.
pub(crate) struct ReleaseReport {
    /// No lease rows remain for the Task: only then may the Box count as Released.
    pub released: bool,
    /// The driver's answer.
    pub driver: Result<Released, DriverError>,
    /// What verify found left behind, plus each failed step's summary.
    pub left: Vec<String>,
}

/// How one script ended.
struct Ran {
    /// Whether it exited 0.
    ok: bool,
    /// What it printed.
    out: String,
    /// Why it did not exit 0.
    why: String,
}

/// A failed step: one summary, plus the things verify found left.
struct Failure {
    /// Why it failed, in plain words.
    summary: String,
    /// What verify found left, when verify failed.
    left: Vec<String>,
}

impl From<String> for Failure {
    fn from(summary: String) -> Self {
        Self {
            summary,
            left: Vec::new(),
        }
    }
}

/// The Mac layout of the Task's Box: the recorded workdir, else what the lease rows name.
fn mac_layout(row: &TaskRow, rows: &[LeaseRow], key: &TaskKey) -> Option<MacLayout> {
    let recorded = row
        .boxed
        .as_ref()
        .and_then(|b| b.workdir.as_deref())
        .and_then(recorded_workdir);
    recorded.map_or_else(
        || layout_from_leases(rows, key),
        |w| MacLayout::find(&w, key),
    )
}

/// Runs every step of the plan; a failed step only warns.
fn run_plan(
    cx: &Cx,
    row: &TaskRow,
    handle: &BoxHandle,
    trust_moor_yaml: bool,
    mac: Option<&MacTarget<'_>>,
    rows: &[LeaseRow],
) -> Outcome {
    let mut outcome = Outcome::default();
    for step in plan(mac, rows) {
        let failure = if step == Step::Teardown {
            teardown(cx, row, trust_moor_yaml);
            None
        } else {
            run_step(cx, row, handle, step, mac, rows)
        };
        outcome.note(step, rows, failure.as_ref());
        if let Some(f) = failure {
            warn_failed(cx, step, &f.summary);
            outcome.fail(f);
        }
    }
    outcome
}

/// Runs the release plan for the Task's Box, records each step on its lease rows and asks the
/// driver to release the Box. A failed step only warns; every later step still runs.
pub(crate) fn release_box(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    handle: &BoxHandle,
    trust_moor_yaml: bool,
) -> Result<ReleaseReport, CommandError> {
    let rows = store.leases(row.id)?;
    let key = TaskKey::new(&row.name);
    let layout = mac_layout(row, &rows, &key);
    let mac = layout
        .as_ref()
        .map(|layout| MacTarget { key: &key, layout });
    let outcome = run_plan(cx, row, handle, trust_moor_yaml, mac.as_ref(), &rows);
    let driver = cx.driver().release(handle);
    let driver_failure = driver
        .as_ref()
        .err()
        .map(|e| shorten(&format!("Box release failed: {e}")));
    record_attempts(store, &rows, &outcome.results, driver_failure.as_deref())?;
    // A failed step's rows go only when the Box was not on a host that outlives the release
    // (non-static, no Mac layout); unreadable caps count as static.
    let done =
        driver.is_ok() && (!outcome.any_failed || (layout.is_none() && !is_static_host(cx, row)));
    if done {
        for r in &rows {
            store.drop_lease(r.id)?;
        }
    }
    Ok(ReleaseReport {
        released: store.leases(row.id)?.is_empty(),
        driver,
        left: outcome.left,
    })
}

/// Whether the Task's Box is on a host that outlives its release; true when caps are unreadable.
fn is_static_host(cx: &Cx, row: &TaskRow) -> bool {
    row.boxed
        .as_ref()
        .is_none_or(|b| !matches!(cx.driver().caps(&b.provider), Ok(c) if !c.static_host))
}

/// Runs one step; `Some` when it failed.
fn run_step(
    cx: &Cx,
    row: &TaskRow,
    handle: &BoxHandle,
    step: Step,
    mac: Option<&MacTarget<'_>>,
    rows: &[LeaseRow],
) -> Option<Failure> {
    let text = script(step, mac, rows)?;
    let ran = run_script(cx, row, handle, &text, step_deadline(step));
    match step {
        Step::Verify => verify(cx, &ran),
        Step::DeleteDevices => {
            let failed = failed_lines(&ran.out);
            if failed.is_empty() {
                (!ran.ok).then(|| summary(&ran).into())
            } else {
                Some(shorten(&failed.join("; ")).into())
            }
        }
        _ => (!ran.ok).then(|| summary(&ran).into()),
    }
}

/// Reads verify output: `left:` lines fail it, `warn:` lines warn.
fn verify(cx: &Cx, ran: &Ran) -> Option<Failure> {
    let report = verify_report(&ran.out);
    for w in report.warnings {
        cx.warn(Warning {
            code: "orphaned_ibtoold".to_owned(),
            message: format!("{w}; moor does not kill an ibtoold it did not start"),
        });
    }
    if report.left.is_empty() {
        return (!ran.ok).then(|| summary(ran).into());
    }
    Some(Failure {
        summary: shorten(&report.left.join("; ")),
        left: report.left,
    })
}

/// The wall-clock limit of one step's script.
fn step_deadline(step: Step) -> Duration {
    match step {
        Step::KillTree | Step::DeleteDevices => SWEEP_DEADLINE,
        Step::StopAgent => {
            Duration::from_secs(u64::from(steps::STOP_AGENT_WAIT_SECS)) + STOP_AGENT_SLACK
        }
        _ => STEP_DEADLINE,
    }
}

/// Runs `script` in the Box over one batched shell, with no env: release runs outside the
/// sandbox, with the real login home, and never inside the Task root.
///
/// The shell starts in `/`. The script's own stdout comes back framed by two nonce markers;
/// anything outside them (a driver banner, say) is never read. Without both markers the step
/// failed, and its summary carries the last lines printed, since a shell syntax error lands there.
fn run_script(cx: &Cx, row: &TaskRow, handle: &BoxHandle, script: &str, deadline: Duration) -> Ran {
    let nonce = format!("{:08x}", cx.rng().u32());
    let host = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let steps = [BoxStep::Run(framed(script, &nonce))];
    let done = cx
        .driver()
        .batch(handle, &[], &steps, host, Deadline(deadline))
        .into_iter()
        .next();
    let (why, stdout) = match done {
        Some(Ok(StepDone::Ran { outcome, stdout })) => (why_not_ok(outcome), stdout),
        Some(Err(e)) => (e.to_string(), String::new()),
        Some(Ok(_)) | None => ("the driver ran no command".to_owned(), String::new()),
    };
    match between_markers(&stdout, &nonce) {
        Some(out) => Ran {
            ok: why.is_empty(),
            out,
            why,
        },
        None => Ran {
            ok: false,
            out: String::new(),
            why: no_output(&why, &stdout, &nonce),
        },
    }
}

/// Why a script with no framed output failed: the exit, then the last lines it printed after
/// the begin marker, or the last lines of everything when that never came.
fn no_output(why: &str, stdout: &str, nonce: &str) -> String {
    let begin = format!("{BEGIN} {nonce}");
    let from = stdout.lines().position(|l| l == begin).map_or(0, |i| i + 1);
    let lines: Vec<&str> = stdout
        .lines()
        .skip(from)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let tail = lines
        .get(lines.len().saturating_sub(TAIL_LINES)..)
        .unwrap_or_default();
    let tail = tail.join("; ");
    let cut = tail.chars().count().saturating_sub(TAIL_CHARS);
    let tail = redact_nonce(&tail.chars().skip(cut).collect::<String>(), None);
    let mut out = "the Box printed no release output".to_owned();
    if !why.is_empty() {
        out = format!("{why}; {out}");
    }
    if !tail.is_empty() {
        out = format!("{out}; last output: {tail}");
    }
    out
}

/// Why an outcome is not a clean exit; empty when it is.
fn why_not_ok(outcome: ExecOutcome) -> String {
    match outcome {
        ExecOutcome::Exited { code: 0 } => String::new(),
        ExecOutcome::Exited { code } => format!("exit {code}"),
        ExecOutcome::Interrupted => "interrupted".to_owned(),
    }
}

/// `script` run from `/` in a subshell, its stdout and stderr between the begin and end marker lines.
fn framed(script: &str, nonce: &str) -> String {
    format!(
        "exec 2>&1\n\
         cd / || exit 1\n\
         echo {BEGIN} {nonce}\n\
         (\n{script}\n)\n\
         c=$?\n\
         echo\n\
         echo {END} {nonce}\n\
         exit $c\n"
    )
}

/// What lies between the marker lines for `nonce`; `None` when either is missing.
fn between_markers(stdout: &str, nonce: &str) -> Option<String> {
    let (begin, end) = (format!("{BEGIN} {nonce}"), format!("{END} {nonce}"));
    let mut lines = stdout.lines().skip_while(|l| *l != begin).skip(1);
    let mut out = String::new();
    for l in lines.by_ref() {
        if l == end {
            return Some(out);
        }
        out.push_str(l);
        out.push('\n');
    }
    None
}

/// The first lines of what a failed script printed, with why it failed.
fn summary(ran: &Ran) -> String {
    let lines: Vec<&str> = ran
        .out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .collect();
    if lines.is_empty() {
        return ran.why.clone();
    }
    shorten(&format!("{} ({})", lines.join("; "), ran.why))
}

/// `text` cut to [`SUMMARY_MAX`] characters.
fn shorten(text: &str) -> String {
    text.chars().take(SUMMARY_MAX).collect()
}

/// Warns that a step failed.
fn warn_failed(cx: &Cx, step: Step, summary: &str) {
    cx.warn(Warning {
        code: "box_release_step_failed".to_owned(),
        message: format!("{} failed: {summary}", step.label()),
    });
}

mod layout;
mod record;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
