//! Starts Boxes pending on a node once it has room for them.

use std::time::Duration;

use moor_core::config::{BoxSettings, Flags};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Decision;
use moor_core::name::NodeName;
use moor_core::{BoxId, BoxName, ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::facts::settings_of;
use crate::ops::common::provision::{provision, reread, Entry};
use crate::ops::common::resume_gate::{room_to_wake, Turn};
use crate::ops::common::wake::{wait_for_room, wake_in_turn};
use crate::ops::start::revive_after_wake;
use crate::store::{BoxRow, Store};

/// How long `close` waits for a waiter's Box lock before skipping that waiter.
pub(crate) const PROMOTE_WAIT: Duration = Duration::from_secs(1);

/// Starts or wakes the Boxes pending on `node`, in queue order, while it has room for them. On a
/// node that reports memory the pass ends at the first Box that still waits for room, so none
/// behind it jumps ahead.
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
    let (all, _) = crate::ops::common::node::nodes(cx)?;
    let reports_memory = all.iter().any(|n| &n.name == node && n.memory.is_some());
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
        match cx.for_box(row.user, || promote_one(cx, store, id, &row.name, op, &cfg))? {
            Promotion::Left(name) => {
                promoted.push(name);
                // A Box that came up since the node's last report is not in it yet; the next report counts it.
                if reports_memory {
                    break;
                }
            }
            Promotion::WaitsForRoom if reports_memory => break,
            Promotion::WaitsForRoom | Promotion::Skipped => {}
        }
    }
    Ok(())
}

/// Gives a Box whose machine vanished while it waited a new one, once its node has room, and
/// brings its agent back as a wake would.
fn replace_machine(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
) -> Result<(), CommandError> {
    if let Some(reason) = room_to_wake(cx, store, row, Turn::Reached)? {
        return Err(wait_for_room(store, row, reason)?);
    }
    store.apply(Decision::new(row.id).lifecycle(BoxLifecycleInput::Wake))?;
    provision(cx, store, row.id, Entry::Stay, cfg)?;
    revive_after_wake(cx, store, row.id)?;
    Ok(())
}

/// What promoting one waiting Box came to.
enum Promotion {
    /// It left Pending.
    Left(BoxName),
    /// Its node has no room for it yet.
    WaitsForRoom,
    /// It could not be tried, or failed for a reason of its own, which holds no other Box back.
    Skipped,
}

/// Promotes one waiting Box under its Box lock.
fn promote_one(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    name: &BoxName,
    op: &str,
    cfg: &BoxSettings,
) -> Result<Promotion, CommandError> {
    let _lock = match BoxLock::acquire(cx.clock(), cx.state_dir()?, id, op, PROMOTE_WAIT) {
        Ok(lock) => lock,
        Err(e) => {
            cx.warn(Warning {
                code: "promote_skipped".to_owned(),
                message: format!("cannot promote {}: {}", name.as_str(), e.error),
            });
            return Ok(Promotion::Skipped);
        }
    };
    let row = reread(store, id)?;
    if row.lifecycle != BoxLifecycle::Pending || row.intent.is_some() {
        return Ok(Promotion::Skipped);
    }
    let asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    let gone = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Gone);
    let started = if asleep {
        wake_in_turn(cx, store, &row).map(|_| ())
    } else if gone {
        replace_machine(cx, store, &row, cfg)
    } else {
        provision(cx, store, id, Entry::Waiting, cfg).map(|_| ())
    };
    if let Err(e) = started {
        let still_waits =
            e.kind == ErrorKind::Refused && reread(store, id)?.lifecycle == BoxLifecycle::Pending;
        if still_waits {
            return Ok(Promotion::WaitsForRoom);
        }
        cx.warn(Warning {
            code: "promote_failed".to_owned(),
            message: format!("cannot promote {}: {}", row.name.as_str(), e.error),
        });
        return Ok(Promotion::Skipped);
    }
    let after = reread(store, id)?;
    Ok(if after.lifecycle == BoxLifecycle::Pending {
        Promotion::WaitsForRoom
    } else {
        Promotion::Left(after.name)
    })
}
