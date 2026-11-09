//! Reconcile stored Boxes against what the Driver still lists.

use moor_core::decide::reconcile::{reconcile, stopped, BoxFact, STOPPED_REASON};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{BoxTarget, Decision, ExtraEvent, Field, Fields};
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::seam::driver::{DriverError, PausePublish};
use moor_core::{BoxId, ProviderId, Warning};

use crate::adapters::proc::plain::last_sentence;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::ops::common::node::now;
use crate::ops::common::stale::unless_stale;
use crate::store::{BoxRow, RunRow, Store, StoreError};

/// Group live Box rows by provider, keeping first-seen order.
fn by_provider(
    rows: Vec<(moor_core::BoxId, crate::store::HandsRow)>,
) -> Vec<(ProviderId, Vec<BoxFact>)> {
    let mut groups: Vec<(ProviderId, Vec<BoxFact>)> = Vec::new();
    for (box_id, b) in rows {
        let fact = BoxFact {
            box_id,
            inc: b.inc,
            state: b.state,
            handle: b.handle,
        };
        match groups.iter_mut().find(|(p, _)| *p == b.provider) {
            Some((_, facts)) => facts.push(fact),
            None => groups.push((b.provider, vec![fact])),
        }
    }
    groups
}

/// Mark Boxes the Driver no longer lists as Gone, and Ready Boxes it lists stopped as Asleep;
/// never provisions.
///
/// A provider that cannot be inventoried is asked about its rows' handles instead, which only
/// sleeps stopped Boxes; one that cannot answer that either is skipped silently. A provider
/// whose listing fails is skipped with a warning; an apply that
/// loses a race with another command is ignored.
pub(crate) fn run(cx: &Cx) {
    let Ok(store) = cx.store() else { return };
    let Ok(rows) = store.box_rows_live() else {
        return;
    };
    sweep(cx, store, rows, &|| false, Verdicts::Any);
}

/// [`run`] for the controller's timer: each verdict is applied only under its Box's turn, so a Box
/// an op is working on is left for the next pass, and the sweep stops once `draining` is true.
/// A provider that can only be asked about handles is left alone, as that call has no deadline.
pub(crate) fn periodic(cx: &Cx, draining: &dyn Fn() -> bool) {
    let Ok(store) = cx.store() else { return };
    let Ok(rows) = store.box_rows_live() else {
        return;
    };
    periodic_over(cx, store, rows, draining);
}

/// [`periodic`] over `rows` read earlier.
pub(crate) fn periodic_over(
    cx: &Cx,
    store: &Store,
    rows: Vec<(BoxId, crate::store::HandsRow)>,
    draining: &dyn Fn() -> bool,
) {
    sweep(cx, store, rows, draining, Verdicts::IdleBoxes);
}

/// Which Boxes a sweep may write a verdict for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdicts {
    /// Any Box, as a read request that may hold the Box's turn itself.
    Any,
    /// Only a Box no op holds the turn of.
    IdleBoxes,
}

/// Lists each provider holding `rows` once and applies what the listing shows.
fn sweep(
    cx: &Cx,
    store: &Store,
    rows: Vec<(BoxId, crate::store::HandsRow)>,
    draining: &dyn Fn() -> bool,
    who: Verdicts,
) {
    for (provider, facts) in by_provider(rows) {
        if draining() {
            return;
        }
        reconcile_provider(cx, store, &provider, &facts, who);
    }
}

/// Reconciles the one Box `row`, when it holds a machine that reconcile can find missing or stopped.
pub(crate) fn one(cx: &Cx, store: &Store, row: &BoxRow) {
    let Some(b) = row
        .hands
        .as_ref()
        .filter(|b| matches!(b.state, HandsState::Ready | HandsState::Asleep))
    else {
        return;
    };
    let fact = BoxFact {
        box_id: row.id,
        inc: b.inc,
        state: b.state,
        handle: b.handle.clone(),
    };
    reconcile_provider(cx, store, &b.provider, &[fact], Verdicts::Any);
}

/// Corrects `facts` from what `provider` lists; see [`run`].
fn reconcile_provider(
    cx: &Cx,
    store: &Store,
    provider: &ProviderId,
    facts: &[BoxFact],
    who: Verdicts,
) {
    match cx.driver().list(provider) {
        Ok(listing) => {
            for d in reconcile(facts, &listing) {
                apply(cx, store, d, who);
            }
        }
        Err(DriverError::Unsupported(_)) if who == Verdicts::IdleBoxes => {}
        Err(DriverError::Unsupported(_)) => {
            let handles: Vec<_> = facts.iter().filter_map(|f| f.handle.clone()).collect();
            if let Ok(listing) = cx.driver().list_handles(&handles) {
                for d in stopped(facts, &listing) {
                    apply(cx, store, d, who);
                }
            }
        }
        Err(e) => {
            note_starting(store, facts, &e);
            let cause = last_sentence(&e.to_string())
                .map(|c| format!(" ({c})"))
                .unwrap_or_default();
            cx.warn(Warning {
                code: "reconcile_skipped".to_owned(),
                message: format!(
                    "could not list boxes for provider {}{cause}; box states left as they were",
                    provider.as_str()
                ),
            });
        }
    }
}

/// Applies `d`. A sleep goes only under the Box's lock on a Box still Ready: a suspend in progress
/// shows its machine as paused before that op records why, and the listing may predate what it
/// recorded. A Box that was quiet just sleeps; one mid-turn has work to carry on, so it queues to
/// resume first, its Session as it was.
fn apply(cx: &Cx, store: &Store, d: Decision, who: Verdicts) {
    let sleep = d.input_to_hands() == Some(&HandsInput::Sleep);
    let (Some(w), BoxTarget::Existing(id), true) = (&d.hands, &d.target, sleep) else {
        let turn = match (who, &d.target) {
            (Verdicts::IdleBoxes, BoxTarget::Existing(id)) => {
                let Some(turn) = cx.try_turn(*id, Cmd::Reconcile) else {
                    return;
                };
                Some(turn)
            }
            _ => None,
        };
        noted(cx, store.apply(d));
        drop(turn);
        return;
    };
    let at = (*id, w.inc);
    let id = *id;
    let Some(_turn) = cx.try_turn(id, Cmd::Reconcile) else {
        return;
    };
    let Ok(Some(row)) = store.box_row(id) else {
        return;
    };
    if row.lifecycle != BoxLifecycle::Ready {
        return;
    }
    let _ = store.set_suspend_check(at, None);
    let _ = store.set_pause_publish(at, PausePublish::Denied);
    if row.session.is_quiet() {
        noted(cx, store.apply(d));
        return;
    }
    let suspend = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Suspend {
            reason: STOPPED_REASON.to_owned(),
        })
        .hands_input(at.1, HandsInput::Sleep)
        .set(Fields {
            state_reason: Field::Set(STOPPED_REASON.to_owned()),
            ..Fields::default()
        });
    if noted(cx, store.apply(suspend)) {
        let _ = store.set_resume_first(id, Some(now(cx)));
    }
}

/// Whether a verdict was written. One about hands the Box has since replaced is dropped with a
/// warning, never retried; any other failure lost a race with another command.
fn noted<T>(cx: &Cx, applied: Result<T, StoreError>) -> bool {
    matches!(unless_stale(cx, applied), Ok(Some(_)))
}

/// Tells each starting Box in `facts` that listing its provider fails, once per distinct error,
/// so its card shows why the provider looks unwell while the acquire still runs.
fn note_starting(store: &Store, facts: &[BoxFact], e: &DriverError) {
    let Some(text) = last_sentence(&e.to_string()) else {
        return;
    };
    for f in facts.iter().filter(|f| f.state == HandsState::Starting) {
        let last = store.last_event_of(f.box_id, "box_progress").ok().flatten();
        if last.is_some_and(|ev| ev.payload.get("text").and_then(|t| t.as_str()) == Some(&text)) {
            continue;
        }
        drop(
            store.apply(
                Decision::new(f.box_id).event(ExtraEvent::BoxProgress { text: text.clone() }),
            ),
        );
    }
}

/// Box `id`'s run in flight. Stale open runs are finished only under the Box's turn; a busy Box,
/// or one whose turn this thread holds, is read without finishing any.
pub(crate) fn run_in_flight(
    cx: &Cx,
    store: &Store,
    id: BoxId,
) -> Result<Option<RunRow>, StoreError> {
    cx.try_turn(id, Cmd::Reconcile)
        .map_or_else(|| store.held_run(id), |_turn| store.in_flight_run(id))
}
