//! The disk half of a node's report: whether its disk is low, and telling each owner once when
//! that changes.

use moor_core::decide::disk::{judge, Change, Report, State};
use moor_core::decide::pressure::Reading;
use moor_core::decide::schedule::Memory;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::name::NodeName;

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// `reading` marked with whether the node's disk is low after it, and the crossing it made.
pub(super) fn judge_disk(
    memory: &Memory,
    last: Option<&Reading>,
    reading: Reading,
    now: i64,
) -> (Reading, Option<Change>) {
    let was = if last.is_some_and(|r| r.disk_low) {
        State::Low
    } else {
        State::Ok
    };
    let (disk, change) = judge(
        was,
        Report {
            at: reading.at,
            free_mib: memory.disk_free_mib,
            total_mib: memory.disk_total_mib,
        },
        now,
    );
    let reading = Reading {
        disk_low: disk == State::Low,
        ..reading
    };
    (reading, change)
}

/// Records `change` of `node`'s disk once for each owner with a Box there, so each hears through
/// the webhooks and streams that already carry Box events. Nothing else happens to the Boxes; a
/// Box with an op in flight is skipped.
pub(super) fn tell_disk(
    cx: &Cx,
    store: &Store,
    node: &NodeName,
    change: Change,
) -> Result<(), CommandError> {
    let ids = store.one_live_per_owner_on(node)?;
    let node = node.to_string();
    for id in ids {
        let event = match change {
            Change::WentLow {
                free_mib,
                total_mib,
            } => ExtraEvent::NodeDiskLow {
                node: node.clone(),
                free_mib,
                total_mib,
            },
            Change::Recovered {
                free_mib,
                total_mib,
            } => ExtraEvent::NodeDiskRecovered {
                node: node.clone(),
                free_mib,
                total_mib,
            },
        };
        if let Some(_turn) = cx.try_turn(id, Cmd::Settle) {
            store.apply(Decision::new(id).event(event))?;
        }
    }
    Ok(())
}
