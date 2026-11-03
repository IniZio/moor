//! Reconcile stored Boxes against what the Driver still lists.

use moor_core::decide::reconcile::{reconcile, stopped, BoxFact};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{BoxTarget, Decision, ExtraEvent};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::DriverError;
use moor_core::{ProviderId, Warning};

use crate::adapters::proc::plain::last_sentence;
use crate::cx::Cx;
use crate::store::Store;

/// Group live Box rows by provider, keeping first-seen order.
fn by_provider(
    rows: Vec<(moor_core::BoxId, crate::store::DriverRow)>,
) -> Vec<(ProviderId, Vec<BoxFact>)> {
    let mut groups: Vec<(ProviderId, Vec<BoxFact>)> = Vec::new();
    for (box_id, b) in rows {
        let fact = BoxFact {
            box_id,
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
    for (provider, facts) in by_provider(rows) {
        match cx.driver().list(&provider) {
            Ok(listing) => {
                for d in reconcile(&facts, &listing) {
                    apply(store, d);
                }
            }
            Err(DriverError::Unsupported(_)) => {
                let handles: Vec<_> = facts.iter().filter_map(|f| f.handle.clone()).collect();
                if let Ok(listing) = cx.driver().list_handles(&handles) {
                    for d in stopped(&facts, &listing) {
                        apply(store, d);
                    }
                }
            }
            Err(e) => {
                note_starting(store, &facts, &e);
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
}

/// Applies `d`; a sleep first quiets the Box's Session, whose agent stopped with the machine.
fn apply(store: &Store, d: Decision) {
    if let (Some(BoxInput::Sleep), BoxTarget::Existing(id)) = (&d.box_input, &d.target) {
        let _ = store.apply(Decision::new(*id).session(SessionInput::AgentQuiet));
    }
    let _ = store.apply(d);
}

/// Tells each starting Box in `facts` that listing its provider fails, once per distinct error,
/// so its card shows why the provider looks unwell while the acquire still runs.
fn note_starting(store: &Store, facts: &[BoxFact], e: &DriverError) {
    let Some(text) = last_sentence(&e.to_string()) else {
        return;
    };
    for f in facts.iter().filter(|f| f.state == BoxState::Starting) {
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
