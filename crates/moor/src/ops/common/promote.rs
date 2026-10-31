//! Starts Boxes pending on a node once it has room for them.

use std::time::Duration;

use moor_core::config::{BoxSettings, Flags};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::name::NodeName;
use moor_core::{BoxId, BoxName, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::facts::settings_of;
use crate::ops::common::provision::{provision, reread, Entry};
use crate::store::Store;

/// How long `close` waits for a waiter's Box lock before skipping that waiter.
pub(crate) const PROMOTE_WAIT: Duration = Duration::from_secs(1);

/// Starts the Boxes pending on `node`, oldest first, while it has room for them.
///
/// Returns the names of the Boxes that left Pending, in order. Best effort: a Box that cannot
/// be started, or an error that stops the pass, becomes a warning and the pass ends with what it did.
pub(crate) fn promote_waiting(cx: &Cx, store: &Store, node: &NodeName, op: &str) -> Vec<BoxName> {
    let mut promoted = Vec::new();
    if let Err(e) = promote_into(cx, store, node, op, &mut promoted) {
        cx.warn(Warning {
            code: "promote_failed".to_owned(),
            message: format!("cannot promote waiting Boxes: {}", e.error),
        });
    }
    promoted
}

/// One promotion pass; `promoted` keeps what was promoted before an error.
fn promote_into(
    cx: &Cx,
    store: &Store,
    node: &NodeName,
    op: &str,
    promoted: &mut Vec<BoxName>,
) -> Result<(), CommandError> {
    for id in store.pending_on(node)? {
        let Some(row) = store.box_row(id)? else {
            continue;
        };
        let cfg = match settings_of(cx, &row, &Flags::default()) {
            Ok(cfg) => cfg,
            Err(e) => {
                cx.for_box(row.user, || {
                    cx.warn(Warning {
                        code: "promote_skipped".to_owned(),
                        message: format!("cannot promote {}: {}", row.name.as_str(), e.error),
                    });
                });
                continue;
            }
        };
        if let Some(name) =
            cx.for_box(row.user, || promote_one(cx, store, id, &row.name, op, &cfg))?
        {
            promoted.push(name);
        }
    }
    Ok(())
}

/// Promotes one waiting Box under its Box lock; `None` when it stays waiting.
fn promote_one(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    name: &BoxName,
    op: &str,
    cfg: &BoxSettings,
) -> Result<Option<BoxName>, CommandError> {
    let _lock = match BoxLock::acquire(cx.clock(), cx.state_dir()?, id, op, PROMOTE_WAIT) {
        Ok(lock) => lock,
        Err(e) => {
            cx.warn(Warning {
                code: "promote_skipped".to_owned(),
                message: format!("cannot promote {}: {}", name.as_str(), e.error),
            });
            return Ok(None);
        }
    };
    let row = reread(store, id)?;
    if row.lifecycle != BoxLifecycle::Pending || row.intent.is_some() {
        return Ok(None);
    }
    if let Err(e) = provision(cx, store, id, Entry::Waiting, cfg) {
        cx.warn(Warning {
            code: "promote_failed".to_owned(),
            message: format!("cannot promote {}: {}", row.name.as_str(), e.error),
        });
        return Ok(None);
    }
    let after = reread(store, id)?;
    Ok((after.lifecycle != BoxLifecycle::Pending).then_some(after.name))
}
