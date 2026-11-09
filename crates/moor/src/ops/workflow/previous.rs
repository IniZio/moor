//! Closing a workflow's earlier Boxes once its next run has a Box, so each workflow and repo
//! holds one Box for follow-ups and its disk does not pile up night after night.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::hands::HandsState;
use moor_core::domain::session_state::SessionState;
use moor_core::request::CloseReq;
use moor_core::BoxId;

use super::StartJob;
use crate::cx::Cx;
use crate::ops::close;
use crate::store::{BoxRow, Store};

/// How many of a repo's latest runs are searched for Boxes to close.
const LOOKBACK: u64 = 50;

/// From this many runs in a row keeping the same Box, its keep asks the owner to look.
const KEEPS_TO_FLAG: usize = 3;

/// Why `row` stays open. Only an asleep Box whose work check, stored when it went to sleep, found
/// nothing unpushed may close: nobody is using it, and every wake clears that check.
pub(super) fn kept(store: &Store, row: &BoxRow) -> Option<String> {
    let asleep = row.lifecycle == BoxLifecycle::Asleep
        && row
            .hands
            .as_ref()
            .is_some_and(|b| b.state == HandsState::Asleep);
    if matches!(row.session, SessionState::Working | SessionState::Starting) {
        return Some("its agent is working".to_owned());
    }
    if !asleep {
        return Some("awake".to_owned());
    }
    match store.suspend_check(row.id) {
        Ok(None) => Some("no stored work check".to_owned()),
        Ok(Some(c)) if c.ahead > 0 || c.dirty > 0 => Some("unpushed work".to_owned()),
        Ok(Some(_)) => None,
        Err(e) => Some(format!("could not read its work check: {e}")),
    }
}

/// Closes the open Box of each run of `job`'s workflow and repo due before `job`, without force
/// and without waking it, when [`kept`] finds no reason to keep it under the Box's lock. Each Box
/// kept gets a `previous_box_kept` event on `new_box` and a line in the returned log; from the
/// third run in a row to keep the same Box, both ask the owner to look.
pub(super) fn close_previous(
    cx: &Cx,
    store: &Store,
    job: &StartJob,
    new_box: BoxId,
) -> Vec<String> {
    let mut out = Vec::new();
    let runs = match store.recent_workflow_runs(job.workflow, &job.repo, LOOKBACK) {
        Ok(runs) => runs,
        Err(e) => {
            return vec![format!(
                "could not list the earlier runs of {}: {e}",
                job.repo
            )]
        }
    };
    // The Boxes of the runs between this one and the one being looked at, newest first.
    let mut newer = Vec::new();
    for run in runs.into_iter().filter(|r| r.due_at < job.due_at) {
        let Some(id) = run.outcome.box_id().filter(|id| *id != new_box) else {
            continue;
        };
        let row = match store.box_row(id) {
            Ok(Some(row)) if row.lifecycle.is_live() => Some(row),
            Ok(_) => None,
            Err(e) => {
                out.push(format!("could not read the earlier Box {id:?}: {e}"));
                None
            }
        };
        if let Some(why) = row.and_then(|row| keep_or_close(cx, &row).map(|w| (row, w))) {
            out.extend(record_keep(store, new_box, &newer, why));
        }
        newer.push(id);
    }
    out
}

/// Closes `row` unwoken unless [`kept`] says otherwise; why it stayed open, if it did.
pub(super) fn keep_or_close(cx: &Cx, row: &BoxRow) -> Option<String> {
    let req = CloseReq {
        target: row.name.clone().into(),
        force: false,
    };
    cx.for_box(row.user, || close::run_unless_kept(cx, &req, &kept))
        .err()
        .map(|e| e.error.to_string())
}

/// Records on `new_box` that `row` was kept for `why`, counting the runs in a row whose Boxes,
/// `newer` newest first, already kept it; the lines to log.
fn record_keep(
    store: &Store,
    new_box: BoxId,
    newer: &[BoxId],
    (row, why): (BoxRow, String),
) -> Vec<String> {
    let name = row.name.as_str();
    let keeps = 1 + newer
        .iter()
        .take_while(|b| kept_before(store, **b, name))
        .count();
    let reason = match keeps {
        1 => why,
        n if n < KEEPS_TO_FLAG => format!("{why}; kept by {n} runs in a row"),
        n => format!("needs attention: {why}; kept by {n} runs in a row"),
    };
    let mut out = vec![format!(
        "kept {name} open, it holds an earlier run: {reason}"
    )];
    let event = ExtraEvent::PreviousBoxKept {
        name: name.to_owned(),
        reason,
    };
    if let Err(e) = store.apply(Decision::new(new_box).event(event)) {
        out.push(format!("could not record the kept Box: {e}"));
    }
    out
}

/// Whether the run of Box `by` kept the Box named `name`.
fn kept_before(store: &Store, by: BoxId, name: &str) -> bool {
    store
        .newest_events_of(by, "previous_box_kept", 100)
        .is_ok_and(|events| {
            events
                .iter()
                .any(|e| e.payload.get("name").and_then(|n| n.as_str()) == Some(name))
        })
}
