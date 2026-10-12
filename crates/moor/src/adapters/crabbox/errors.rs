//! Maps a failed `crabbox` run to a `DriverError`, once.
//!
//! crabbox never prints JSON errors: the message is free text on stderr and
//! the exit code is coarse. Raw text is read for classification only and is
//! never copied into the returned error.

use moor_core::seam::driver::DriverError;
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
    /// Reading a Box's working directory.
    Workdir,
    /// Reading provider facts.
    Caps,
    /// Running a command in a Box.
    Exec,
    /// Copying a file out of a Box.
    CopyOut,
}

impl Op {
    /// Moor wording for the operation.
    const fn phrase(self) -> &'static str {
        match self {
            Self::Acquire => "box acquire",
            Self::Release => "box release",
            Self::List => "box list",
            Self::Workdir => "box workdir lookup",
            Self::Caps => "provider lookup",
            Self::Exec => "box command",
            Self::CopyOut => "box copy out",
        }
    }
}

/// Exit code for a refused request.
const EXIT_REFUSED: i32 = 4;
/// Exit code for a Box lost during warmup.
const EXIT_LOST: i32 = 5;
/// Exit code for a configuration problem.
const EXIT_CONFIG: i32 = 3;

/// Whether `hay` contains any of `needles`.
fn has_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Classifies lowercase stderr into a typed error, when it names one.
fn classify_text(code: i32, text: &str) -> Option<DriverError> {
    if code == EXIT_REFUSED && text.contains("not found") {
        return Some(DriverError::NotFound);
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
pub(super) fn remote_exit(e: &ProcError) -> Option<i32> {
    let ProcError::NonZero {
        code, stderr_tail, ..
    } = e
    else {
        return None;
    };
    let line = format!("remote command exited {code}");
    stderr_tail
        .lines()
        .any(|l| l.trim_end().ends_with(&line))
        .then_some(*code)
}

/// Maps a failed run of `op` (on `provider`, when known) to the one error shape callers see.
pub(super) fn map_error(op: Op, provider: Option<&str>, e: &ProcError) -> DriverError {
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
            let summary = match *code {
                EXIT_CONFIG => format!("{what} failed (crabbox configuration problem)"),
                EXIT_LOST => format!("{what} failed (box lost during warmup)"),
                _ => provider.map_or_else(
                    || format!("{what} failed (crabbox exit {code})"),
                    |p| format!("{what} failed on provider {p} (exit {code})"),
                ),
            };
            DriverError::Failed { summary }
        }),
    }
}

#[cfg(test)]
mod tests;
