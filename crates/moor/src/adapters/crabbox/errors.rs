//! Maps a failed `crabbox` run to a `DriverError`, once.
//!
//! crabbox never prints JSON errors: the message is free text on stderr and
//! the exit code is coarse. Raw text is read for classification; only the
//! generic acquire, release and provider lookup failures carry a short stderr tail.

use moor_core::seam::driver::{DriverError, TunnelFailure};
use moor_core::seam::proc::ProcError;

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
    /// Copying a file out of a Box.
    CopyOut,
    /// Looking up a Box's ssh target.
    Target,
    /// Suspending a Box.
    Sleep,
    /// Resuming a Box.
    Wake,
}

impl Op {
    /// Moor wording for the operation.
    const fn phrase(self) -> &'static str {
        match self {
            Self::Acquire => "box acquire",
            Self::Release => "box release",
            Self::List => "box list",
            Self::Caps => "provider lookup",
            Self::CopyOut => "box copy out",
            Self::Target => "box ssh target lookup",
            Self::Sleep => "box sleep",
            Self::Wake => "box wake",
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
/// Lowercase words ssh prints when the connection to the Box drops or never comes up.
const SSH_LOST: &[&str] = &[
    "closed by remote host",
    "connection reset",
    "broken pipe",
    "kex_exchange_identification",
    "connection timed out",
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

/// Longest stderr tail, in characters, appended to a summary.
const TAIL_MAX: usize = 300;

/// The stderr tail appended to a summary of `op`, or nothing.
fn stderr_suffix(op: Op, stderr: &str) -> String {
    if !matches!(op, Op::Acquire | Op::Release | Op::Caps) {
        return String::new();
    }
    let line = one_line(stderr);
    if line.is_empty() {
        String::new()
    } else {
        format!(": {line}")
    }
}

/// The end of `stderr` as one line: trimmed non-empty lines joined by `; `, cut from the front to `TAIL_MAX`.
pub(super) fn one_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let joined = lines.join("; ");
    let count = joined.chars().count();
    if count <= TAIL_MAX {
        return joined;
    }
    let tail: String = joined.chars().skip(count - (TAIL_MAX - 1)).collect();
    format!("…{}", tail.trim_start())
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
        } => classify_text(*code, &stderr_tail.to_lowercase()).unwrap_or_else(|| {
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

#[cfg(test)]
mod tests;
