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

/// Maps a failed run of `op` to the one error shape callers see.
pub(super) fn map_error(op: Op, e: &ProcError) -> DriverError {
    let what = op.phrase();
    match e {
        ProcError::Spawn(_) => DriverError::Unavailable("crabbox could not be started".to_owned()),
        ProcError::Timeout { after, .. } => DriverError::Timeout(*after),
        ProcError::Signalled(_) => DriverError::Failed {
            summary: format!("{what} failed (crabbox was killed)"),
        },
        ProcError::NonZero {
            code, stderr_tail, ..
        } => classify_text(*code, &stderr_tail.to_lowercase()).unwrap_or_else(|| {
            let summary = match *code {
                EXIT_CONFIG => format!("{what} failed (crabbox configuration problem)"),
                EXIT_LOST => format!("{what} failed (box lost during warmup)"),
                _ => format!("{what} failed (crabbox exit {code})"),
            };
            DriverError::Failed { summary }
        }),
    }
}

#[cfg(test)]
mod tests;
