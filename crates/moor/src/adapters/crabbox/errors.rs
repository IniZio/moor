//! Maps a failed `crabbox` run to a `DriverError`, once.
//!
//! crabbox never prints JSON errors: the message is free text on stderr and
//! the exit code is coarse. Raw text is read for classification; only the
//! generic acquire, release, checkpoint and provider lookup failures carry a short stderr
//! tail. A failed acquire, release or checkpoint also logs the plain words of each stderr line
//! moor kept, which is the last 2 KiB.

use std::fmt::Write as _;

use moor_core::seam::driver::{DriverError, TunnelFailure};
use moor_core::seam::proc::{ProcError, ProcOutput, ProcSpec};

use super::Crabbox;
use crate::adapters::proc::plain::last_sentence;

/// The crabbox operation that failed.
#[derive(Clone, Copy, Debug)]
pub(super) enum Op {
    /// Creating a Box.
    Acquire,
    /// Deleting a Box.
    Release,
    /// Listing Boxes.
    List,
    /// Reading provider facts.
    Caps,
    /// Looking up a Box's ssh target.
    Target,
    /// Suspending a Box.
    Sleep,
    /// Resuming a Box.
    Wake,
    /// Touching a Box's lease.
    Touch,
    /// Saving a Box's checkpoint.
    Checkpoint,
}

impl Op {
    /// Moor wording for the operation.
    const fn phrase(self) -> &'static str {
        match self {
            Self::Acquire => "box acquire",
            Self::Touch => "box lease touch",
            Self::Release => "box release",
            Self::List => "box list",
            Self::Caps => "provider lookup",
            Self::Target => "box ssh target lookup",
            Self::Sleep => "box sleep",
            Self::Wake => "box wake",
            Self::Checkpoint => "box checkpoint",
        }
    }
}

/// Exit code for a refused request.
const EXIT_REFUSED: i32 = 4;
/// Exit code for a Box lost during warmup.
const EXIT_LOST: i32 = 5;
/// Exit code for a configuration problem.
const EXIT_CONFIG: i32 = 3;

/// Exit code crabbox uses for a lease claim that changed during the call.
const EXIT_CLAIM_RACE: i32 = 2;
/// Words crabbox prints when another call rewrote the lease claim.
const CLAIM_RACE: &str = "claim changed";

/// Whether `hay` contains any of `needles`.
fn has_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Words crabbox prints when a stop has no lease to resolve, so nothing was ever acquired.
const NO_LEASE: &str = "resolve needs a cloud id or a desired lease";

/// Classifies lowercase stderr of `op` into a typed error, when it names one.
///
/// A provider other than crabbox's own reports a lease it never made or already dropped as
/// `lease <id> not found` under whatever exit code its command uses, so for a release the words
/// decide.
fn classify(op: Op, code: i32, text: &str) -> Option<DriverError> {
    if matches!(op, Op::Release)
        && ((text.contains("lease") && text.contains("not found")) || text.contains(NO_LEASE))
    {
        return Some(DriverError::NotFound);
    }
    classify_text(code, text)
}

/// Classifies lowercase stderr into a typed error, when it names one.
fn classify_text(code: i32, text: &str) -> Option<DriverError> {
    if code == EXIT_REFUSED && text.contains("not found") {
        return Some(DriverError::NotFound);
    }
    if code == EXIT_CLAIM_RACE && text.contains(CLAIM_RACE) {
        return Some(DriverError::Failed {
            summary: "the Box lease was busy; try again".to_owned(),
        });
    }
    if text.contains("local git seed rev-list failed") {
        return Some(DriverError::Failed {
            summary: "the host checkout is shallow, so crabbox could not seed Git from it; \
                      run `git fetch --unshallow` in the host checkout and retry"
                .to_owned(),
        });
    }
    if text.contains("quota") {
        return Some(DriverError::Quota("the plan limit was reached".to_owned()));
    }
    if has_any(
        text,
        &[
            "unauthorized",
            "authentication",
            "forbidden",
            "invalid token",
        ],
    ) {
        return Some(DriverError::Auth(
            "the provider rejected the credentials".to_owned(),
        ));
    }
    if has_any(text, &["capacity", "too many leases", "lease limit"]) {
        return Some(DriverError::Capacity {
            limit: None,
            holder: None,
        });
    }
    None
}

/// The exit code of the remote command, when `e` reports one.
///
/// crabbox prints `remote command exited <code>` on its own line. The line must
/// end with exactly the process's exit code, so `17` never matches `1`.
///
/// A transport loss is not a remote exit, though crabbox prints ssh's 255 as one.
pub(super) fn remote_exit(e: &ProcError) -> Option<i32> {
    let ProcError::NonZero {
        code, stderr_tail, ..
    } = e
    else {
        return None;
    };
    if transport_loss(e).is_some() {
        return None;
    }
    let line = format!("remote command exited {code}");
    stderr_tail
        .lines()
        .any(|l| l.trim_end().ends_with(&line))
        .then_some(*code)
}

/// Exit code ssh uses for its own failures; crabbox reports it as the remote command's exit.
pub(super) const EXIT_SSH: i32 = 255;
/// Lowercase words ssh or its proxy print when the connection to the Box drops or never comes up.
const SSH_LOST: &[&str] = &[
    "closed by remote host",
    "connection reset",
    "broken pipe",
    "kex_exchange_identification",
    "connection timed out",
    "is not linked",
];

/// The transport error when `e` is ssh losing the connection to the Box.
///
/// A command that exits 255 without ssh's words on stderr is the command's own exit. crabbox's
/// `remote command exited 255` line is left out of the tail, since no exit is known.
pub(super) fn transport_loss(e: &ProcError) -> Option<DriverError> {
    let ProcError::NonZero {
        code: EXIT_SSH,
        stderr_tail,
        ..
    } = e
    else {
        return None;
    };
    let exit_line = format!("remote command exited {EXIT_SSH}");
    let ssh_text = stderr_tail
        .lines()
        .filter(|l| !l.trim_end().ends_with(&exit_line))
        .collect::<Vec<_>>()
        .join("\n");
    has_any(&ssh_text.to_lowercase(), SSH_LOST).then(|| DriverError::Transport {
        stderr_tail: one_line(&ssh_text),
    })
}

/// Whether `e` is crabbox giving up because another call rewrote the lease claim.
///
/// crabbox reports this while it resolves the lease, before the remote command
/// starts, so a remote exit line rules the failure out.
pub(super) fn is_claim_race(e: &ProcError) -> bool {
    let ProcError::NonZero {
        code, stderr_tail, ..
    } = e
    else {
        return false;
    };
    *code == EXIT_CLAIM_RACE
        && remote_exit(e).is_none()
        && stderr_tail.to_lowercase().contains(CLAIM_RACE)
}

/// Whether a reverse tunnel exited before its ready line because another call rewrote the lease claim.
pub(super) fn is_tunnel_claim_race(e: &DriverError) -> bool {
    matches!(e, DriverError::Tunnel {
        failure: TunnelFailure::Exited { code: Some(EXIT_CLAIM_RACE), signal: None },
        stderr_tail,
    } if stderr_tail.to_lowercase().contains(CLAIM_RACE))
}

/// The stderr tail appended to a summary of `op`, or nothing.
fn stderr_suffix(op: Op, stderr: &str) -> String {
    if matches!(op, Op::List) {
        return last_sentence(stderr).map_or_else(String::new, |s| format!(": {s}"));
    }
    if !matches!(op, Op::Acquire | Op::Release | Op::Caps | Op::Checkpoint) {
        return String::new();
    }
    let line = one_line(stderr);
    if line.is_empty() {
        String::new()
    } else {
        format!(": {line}")
    }
}

pub(super) use tail::one_line;

/// The plain words of each stderr line a failed acquire, release or checkpoint left, for the log.
pub(super) fn stderr_record(op: Op, provider: Option<&str>, e: &ProcError) -> Option<String> {
    if !matches!(op, Op::Acquire | Op::Release | Op::Checkpoint) {
        return None;
    }
    let ProcError::NonZero {
        code, stderr_tail, ..
    } = e
    else {
        return None;
    };
    let lines: Vec<String> = stderr_tail.lines().filter_map(last_sentence).collect();
    if lines.is_empty() {
        return None;
    }
    let on = provider.map_or_else(String::new, |p| format!(" on provider {p}"));
    let what = op.phrase();
    let mut record = String::new();
    for line in lines {
        let _ = writeln!(
            record,
            "{what} failed{on} (exit {code}), crabbox said: {line}"
        );
    }
    Some(record)
}

/// Writes what a failed `op` printed to `log`, then maps it.
fn log_error(
    op: Op,
    provider: Option<&str>,
    e: &ProcError,
    log: &mut dyn std::io::Write,
) -> DriverError {
    if let Some(record) = stderr_record(op, provider, e) {
        drop(log.write_all(record.as_bytes()));
    }
    map_error(op, provider, e)
}

impl Crabbox {
    /// Runs one crabbox call, mapping failure through `op`.
    pub(super) fn call(
        &self,
        op: Op,
        provider: Option<&str>,
        spec: &ProcSpec,
    ) -> Result<ProcOutput, DriverError> {
        self.proc
            .run(spec)
            .map_err(|e| self.failed(op, provider, &e))
    }

    /// Maps a failed run of `op`, writing what it printed to the failure log.
    pub(super) fn failed(&self, op: Op, provider: Option<&str>, e: &ProcError) -> DriverError {
        let mut log = self
            .failure_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log_error(op, provider, e, &mut *log)
    }
}

/// Maps a failed run of `op` (on `provider`, when known) to the one error shape callers see.
pub(super) fn map_error(op: Op, provider: Option<&str>, e: &ProcError) -> DriverError {
    if let Some(lost) = transport_loss(e) {
        return lost;
    }
    let what = op.phrase();
    match e {
        ProcError::Spawn(_) => DriverError::Unavailable("crabbox could not be started".to_owned()),
        ProcError::Timeout { after, .. } => DriverError::Timeout(*after),
        ProcError::Signalled(_) | ProcError::Interrupted => DriverError::Failed {
            summary: format!("{what} failed (crabbox was killed)"),
        },
        ProcError::NonZero {
            code, stderr_tail, ..
        } => routing_refusal(stderr_tail)
            .or_else(|| classify(op, *code, &stderr_tail.to_lowercase()))
            .unwrap_or_else(|| {
                let tail = stderr_suffix(op, stderr_tail);
                let summary = match *code {
                    EXIT_CONFIG => format!("{what} failed (crabbox configuration problem){tail}"),
                    EXIT_LOST => format!("{what} failed (box lost during warmup)"),
                    _ => provider.map_or_else(
                        || format!("{what} failed (crabbox exit {code}){tail}"),
                        |p| format!("{what} failed on provider {p} (exit {code}){tail}"),
                    ),
                };
                DriverError::Failed { summary }
            }),
    }
}

/// The error for crabbox refusing its external routing directory, which no retry changes.
///
/// crabbox names only the one path component it refused, so the fix is said for that name.
fn routing_refusal(stderr: &str) -> Option<DriverError> {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| l.to_lowercase().contains("external routing directory"))?;
    let lower = line.to_lowercase();
    let fix = if lower.contains("must not be writable by group or others") {
        "run `chmod go-w` on that directory"
    } else if lower.contains("without symlinks") {
        "replace the symlink on that path with the real directory"
    } else {
        return None;
    };
    Some(DriverError::Misconfigured {
        summary: format!("crabbox refused its external routing directory: {line}; {fix}"),
    })
}

/// Words the provider prints when a fork finds its checkpoint was never finished.
const UNFINISHED: &str = "was never finished";

/// `e` as [`DriverError::UnfinishedCheckpoint`] when a failed fork says its checkpoint was never
/// finished; crabbox has no other way to say so.
pub(super) fn unfinished_fork(e: DriverError) -> DriverError {
    match e {
        DriverError::Failed { summary } if summary.to_lowercase().contains(UNFINISHED) => {
            DriverError::UnfinishedCheckpoint { summary }
        }
        e => e,
    }
}

/// Error for output moor cannot read.
pub(super) fn unreadable(what: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("unreadable box provider output for {what}"),
    }
}

mod tail;
#[cfg(test)]
mod tests;
