//! Keep the codes machines pair by in the store, so a code one gate replica handed out can be
//! confirmed and collected on any other.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `machine_pairings` table.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("machine_pairings")
            .col(text("code").primary_key())
            .col(text("device_hash").not_null().unique_key())
            .col(text("ask").not_null())
            .col(text("stage").not_null())
            .col(text("enrolled"))
            .col(int("expires_at").not_null()),
    );
}
