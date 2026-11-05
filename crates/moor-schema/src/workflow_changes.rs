//! Gives workflows an origin and every run and disabling a position in one change stream.
//!
//! One row holds the stream's counter, made by the first change. Each change takes the next
//! number inside its own transaction, and the row stays locked until that transaction ends, so
//! changes commit in the order they are numbered and a reader that has seen a number has seen
//! every smaller one.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the origin and sequence columns and the counter table.
pub(super) fn write(s: &mut Script) {
    for col in [
        text("origin_front"),
        text("origin_key"),
        int("change_seq").not_null().default(0).take(),
    ] {
        s.schema(&Table::alter().table("workflows").add_column(col).take());
    }
    s.schema(
        &Table::alter()
            .table("workflow_runs")
            .add_column(int("seq").not_null().default(0).take())
            .take(),
    );
    s.schema(
        Table::create()
            .table("workflow_seq")
            .col(int("id").not_null().primary_key())
            .col(int("n").not_null()),
    );
}
