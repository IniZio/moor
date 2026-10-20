//! Reconcile stored Boxes against what the Driver still lists.

use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::seam::driver::DriverError;
use moor_core::{ProviderId, Warning};

use crate::cx::Cx;

/// Group live Box rows by provider, keeping first-seen order.
fn by_provider(
    rows: Vec<(moor_core::BoxId, crate::store::DriverRow)>,
) -> Vec<(ProviderId, Vec<BoxFact>)> {
    let mut groups: Vec<(ProviderId, Vec<BoxFact>)> = Vec::new();
    for (task, b) in rows {
        let fact = BoxFact {
            box_id: task,
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
            Err(_) => cx.warn(Warning {
                code: "reconcile_skipped".to_owned(),
                message: format!(
                    "could not list boxes for provider {}; box states left as they were",
                    provider.as_str()
                ),
            }),
        }
    }
}
