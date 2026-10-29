//! Reconcile stored Boxes against what the Driver still lists.

use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
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

/// Mark Boxes the Driver no longer lists as Gone; never provisions.
///
/// A provider that cannot be inventoried is skipped silently; a provider
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
                    let _ = store.apply(d);
                }
            }
            Err(DriverError::Unsupported(_)) => {}
            Err(e) => {
                note_starting(store, &facts, &e);
                cx.warn(Warning {
                    code: "reconcile_skipped".to_owned(),
                    message: format!(
                        "could not list boxes for provider {}; box states left as they were",
                        provider.as_str()
                    ),
                });
            }
        }
    }
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
