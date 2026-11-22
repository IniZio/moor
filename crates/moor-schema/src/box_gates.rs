//! Let a Box carry gates, and keep the lines that say how far they have got.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk_cascade, int, key, text};

/// Add the boxes' `gates` column and the gate lines table.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(text("gates"))
            .take(),
    );
    s.schema(
        Table::create()
            .table("box_gate_lines")
            .col(int("box").not_null())
            .col(int("n").not_null())
            .col(int("at").not_null())
            .col(text("entry").not_null())
            .index(key(&["box", "n"]).unique())
            .foreign_key(&mut fk_cascade("box", "boxes", "id")),
    );
}
