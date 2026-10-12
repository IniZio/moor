//! Gate verdicts from recorded runs.

use crate::name::RunId;
use crate::response::ErrorKind;
use crate::seam::clock::Ts;

/// Shortest sha prefix a query may use.
const MIN_PREFIX: usize = 4;
/// Shortest prefix whose miss is a failure rather than a usage error.
const FULL_PREFIX: usize = 7;

/// One recorded run, as the store read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRun {
    /// The run's id.
    pub id: RunId,
    /// Full commit sha the run was recorded against.
    pub sha: String,
    /// Record name (the gate name).
    pub name: String,
    /// The command text that ran.
    pub command: String,
    /// Exit status; `None` when the process never reported one.
    pub exit: Option<i32>,
    /// Whether the run was interrupted.
    pub interrupted: bool,
    /// When the run was recorded.
    pub recorded_at: Ts,
}

impl GateRun {
    /// A run passes only with exit 0 and no interruption.
    fn passed(&self) -> bool {
        !self.interrupted && self.exit == Some(0)
    }
}

/// What a gate check asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateQuery<'a> {
    /// Commit sha or prefix, any case.
    pub sha: &'a str,
    /// Record name of the gate.
    pub name: &'a str,
    /// Only consider runs of exactly this command text.
    pub command: Option<&'a str>,
}

/// The answer to a gate check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateVerdict {
    /// The resolved full sha, lowercase.
    pub sha: String,
    /// The gate name.
    pub name: String,
    /// Command of the chosen run; `None` when no run matched.
    pub command: Option<String>,
    /// Exit of the chosen run; `None` when none matched or it had none.
    pub exit: Option<i32>,
    /// Whether the gate passed.
    pub ok: bool,
    /// When the chosen run was recorded.
    pub recorded_at: Option<Ts>,
}

/// Why a gate check cannot answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// The sha query is malformed, too short or ambiguous.
    #[error("{0}")]
    Usage(String),
    /// A full-length sha matches no recorded run.
    #[error("no recorded run for commit {0}")]
    Failed(String),
}

impl GateError {
    /// The error kind the response envelope reports.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Usage(_) => ErrorKind::Usage,
            Self::Failed(_) => ErrorKind::Failed,
        }
    }
}

/// Answers whether a recorded run of the gate passed on the commit.
///
/// `runs` holds recorded runs of every name: the sha prefix resolves
/// against all of them, so an unknown commit stays distinct from a known
/// commit that never ran this gate. Any passing run beats a newer
/// failing one; among equals the latest recording wins.
///
/// # Errors
///
/// [`GateError::Usage`] for a malformed, short or ambiguous prefix, or an
/// unknown prefix under seven characters; [`GateError::Failed`] for an
/// unknown prefix of seven or more.
pub fn decide(q: &GateQuery<'_>, runs: &[GateRun]) -> Result<GateVerdict, GateError> {
    let prefix = q.sha.to_ascii_lowercase();
    if prefix.len() < MIN_PREFIX || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GateError::Usage(format!(
            "commit must be at least {MIN_PREFIX} hex characters, got `{}`",
            q.sha
        )));
    }
    let mut full: Option<String> = None;
    for run in runs {
        let sha = run.sha.to_ascii_lowercase();
        if !sha.starts_with(&prefix) {
            continue;
        }
        match &full {
            Some(seen) if *seen != sha => {
                return Err(GateError::Usage(format!(
                    "commit prefix `{}` is ambiguous; use more characters",
                    q.sha
                )));
            }
            Some(_) => {}
            None => full = Some(sha),
        }
    }
    let Some(sha) = full else {
        return Err(if prefix.len() < FULL_PREFIX {
            GateError::Usage(format!("no recorded commit starts with `{}`", q.sha))
        } else {
            GateError::Failed(prefix)
        });
    };
    let chosen = runs
        .iter()
        .filter(|r| r.sha.eq_ignore_ascii_case(&sha) && r.name == q.name)
        .filter(|r| q.command.is_none_or(|c| r.command == c))
        .max_by_key(|r| (r.passed(), r.recorded_at, r.id));
    Ok(GateVerdict {
        sha,
        name: q.name.to_owned(),
        command: chosen.map(|r| r.command.clone()),
        exit: chosen.and_then(|r| r.exit),
        ok: chosen.is_some_and(GateRun::passed),
        recorded_at: chosen.map(|r| r.recorded_at),
    })
}

#[cfg(test)]
mod tests;
