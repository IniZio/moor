//! The gate lines in the history of their Box's run.
//!
//! A gate line writes its history lines in its own transaction, so the history reads in the order
//! things happened and a line written twice, which fails on its number, writes them once.

use moor_core::seam::clock::Ts;
use moor_core::workflow::gate_history;
use moor_core::BoxId;

use super::waiting::load;
use crate::store::db::Tx;
use crate::store::users::workflow::append_to_open_run;
use crate::store::StoreError;

/// Writes what the line just appended to Box `id` adds to the history of its run.
pub(super) fn note(tx: &Tx<'_>, id: BoxId, at: Ts) -> Result<(), StoreError> {
    let Some((gates, lines)) = load(tx, id)? else {
        return Ok(());
    };
    let Some((line, before)) = lines.split_last() else {
        return Ok(());
    };
    append_to_open_run(tx, id, at, |waits| {
        gate_history(&gates, before, line, waits)
    })
}
