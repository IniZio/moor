//! What a delivered run found, as the pages name it: a run of a gated Box ends in one outcome,
//! and a bad one ("check failed") is still a run that did its job, not a failure.

use moor_core::gate::{Verdict, CLOSING};
use moor_core::view::workflow::LastRunView;
use serde_json::Value;

/// The outcome the run ended in, unless it is the implicit `done` of a workflow that names none,
/// which a person reads as the run finishing.
pub(super) fn named(run: &LastRunView) -> Option<&Verdict> {
    run.verdict.as_ref().filter(|v| !v.is_implicit())
}

/// What the run shows beside its icon: the outcome's name, else the run's own label.
pub(super) fn label_of(run: &LastRunView) -> String {
    named(run).map_or_else(
        || super::label(&run.phase, &run.outcome, run.step.as_deref()),
        |v| v.name.to_string(),
    )
}

/// The word that ends a run's history, from its `ended` line: the outcome's name when it names one,
/// else the run's own outcome.
pub(super) fn ended_word(entry: &Value) -> String {
    let ended = entry.get("outcome");
    let named = ended
        .and_then(|o| o.get("verdict"))
        .and_then(|v| serde_json::from_value::<Verdict>(v.clone()).ok())
        .filter(|v| !v.is_implicit());
    named.map_or_else(
        || {
            let word = ended.and_then(|o| o.get("outcome")).and_then(Value::as_str);
            word.unwrap_or("done").replace('_', " ")
        },
        |v| v.name.to_string(),
    )
}

/// Whether the run found an outcome, so a gate that has not passed was never reached: an
/// `anytime` outcome held before the run got to it.
pub(super) fn ended_early(run: &LastRunView, gate_lines: &[&Value]) -> bool {
    run.verdict.is_some()
        && !gate_lines
            .iter()
            .any(|e| e.get("kind").and_then(Value::as_str) == Some("gate_passed"))
}

/// Where the run stands at its closing stage: `deciding` while moor tries the outcomes, `waiting`
/// when none held and a person must steer; none before it got there or after it ended.
pub(super) fn closing(run: &LastRunView) -> Option<&'static str> {
    if run.verdict.is_some() || run.phase == "done" {
        return None;
    }
    let at_closing = run.history.iter().rev().find_map(|l| {
        let entry = &l.entry;
        match entry.get("kind").and_then(Value::as_str)? {
            "entered" => Some(entry.get("step").and_then(Value::as_str) == Some(CLOSING)),
            _ => None,
        }
    });
    match (at_closing, run.phase.as_str()) {
        (Some(true), "waiting") => Some("waiting"),
        (Some(true), _) => Some("deciding"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
