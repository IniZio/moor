//! The workflow run a Box came from, so the page can offer to run it again.

use moor_core::name::BoxName;
use moor_core::progress::Progress;
use serde_json::{json, Value};

use super::super::{workflows, Session};
use super::Row;
use crate::Gate;

/// Whether `name` is shaped like the Box of a workflow run, `<workflow>/w<id>-<due time>`.
pub(super) fn names_a_run(name: &str) -> bool {
    let Some(label) = BoxName::parse(name).ok().map(|n| n.label().to_owned()) else {
        return false;
    };
    let digits = label
        .strip_prefix('w')
        .map_or("", |rest| rest.split_once('-').map_or("", |(id, _)| id));
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The workflow of the run that made `row`'s Box, when the Box has stopped working, the run is
/// the workflow's latest and it went wrong; null otherwise. A later run has taken the place of an
/// older one, so there is nothing to run again. Only a Box named like a run reads the workflows.
pub(super) fn failed_run_of(gate: &Gate, session: &Session, row: Option<&Row>) -> Value {
    let Some(row) = row.filter(|r| names_a_run(&r.name)) else {
        return Value::Null;
    };
    let state = row.progress.as_ref().map(|p| p.state);
    let over = matches!(
        state,
        Some(Progress::Failed | Progress::Closed | Progress::Done | Progress::Asleep)
    );
    if !over {
        return Value::Null;
    }
    let all = workflows::listed(gate, session).unwrap_or_default();
    all.iter()
        .find_map(|w| {
            let name = Some(row.name.as_str());
            let run = w
                .last_run
                .as_ref()
                .filter(|r| r.box_name.as_deref() == name || r.planned_box.as_deref() == name)?;
            workflows::went_wrong(run).then(|| json!({"name": w.name}))
        })
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::names_a_run;

    #[test]
    fn only_a_name_shaped_like_a_run_reads_the_workflows() {
        let named = names_a_run;
        assert!(named("nightly/w12-202611190300"));
        assert!(named("nightly/w3-m20261119030012"));
        assert!(!named("web/mcp-1a2b3c4d"));
        assert!(!named("web/wx-1"));
        assert!(!named("web/w-1"));
        assert!(!named("web-1"));
    }
}
