//! What a release records on the lease rows: the failures of the steps that answer on a row.

use crate::error::CommandError;
use crate::store::lease::{LeaseId, LeaseRow};
use crate::store::Store;

use super::steps::{rows_for, Step};
use super::Failure;

/// What the plan's steps did: the failures per lease row, and what was left.
#[derive(Default)]
pub(super) struct Outcome {
    /// Each lease row answered by a step, with the failures of those steps.
    pub(super) results: Vec<(LeaseId, Vec<String>)>,
    /// What verify found left behind, plus each failed step's summary.
    pub(super) left: Vec<String>,
    /// Whether any step failed.
    pub(super) any_failed: bool,
}

impl Outcome {
    /// Notes one step's answer on every lease row it answers on.
    pub(super) fn note(&mut self, step: Step, rows: &[LeaseRow], failure: Option<&Failure>) {
        for id in rows_for(step, rows) {
            let at = self
                .results
                .iter()
                .position(|(i, _)| *i == id)
                .unwrap_or_else(|| {
                    self.results.push((id, Vec::new()));
                    self.results.len() - 1
                });
            if let (Some(f), Some((_, why))) = (failure, self.results.get_mut(at)) {
                why.push(format!("{}: {}", step.label(), f.summary));
            }
        }
    }

    /// Keeps what a failed step left behind.
    pub(super) fn fail(&mut self, f: Failure) {
        self.any_failed = true;
        self.left.extend(if f.left.is_empty() {
            vec![f.summary]
        } else {
            f.left
        });
    }
}

/// Writes one attempt per lease row, with its step failures and the driver's failure when there
/// are any; a row no step answers on still counts the attempt.
pub(super) fn record_attempts(
    store: &Store,
    rows: &[LeaseRow],
    results: &[(LeaseId, Vec<String>)],
    driver_failure: Option<&str>,
) -> Result<(), CommandError> {
    for row in rows {
        let mut why: Vec<&str> = results
            .iter()
            .filter(|(id, _)| *id == row.id)
            .flat_map(|(_, w)| w.iter().map(String::as_str))
            .collect();
        why.extend(driver_failure);
        let failure = (!why.is_empty()).then(|| why.join("; "));
        store.record_attempt(row.id, failure.as_deref())?;
    }
    Ok(())
}
