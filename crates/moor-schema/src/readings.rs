//! Keep what moor made of a prompt: one row per reading, and the reading an automation holds.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk_cascade, index, int, text};

/// Add the `readings` table and the workflows' `reading`: the id of the reading the automation
/// holds, null for none. Nothing is rewritten, so a workflow keeps its gates and no row is added.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("readings")
            .col(text("id").primary_key())
            .col(int("of_workflow"))
            .col(int("owner").not_null())
            .col(text("parent"))
            .col(text("source").not_null())
            .col(text("context").not_null())
            .col(text("state").not_null())
            .col(text("reader_box"))
            .col(int("since").not_null())
            .col(text("failed"))
            .col(text("read"))
            .foreign_key(&mut fk_cascade("of_workflow", "workflows", "id"))
            .foreign_key(&mut fk_cascade("owner", "users", "id")),
    );
    s.schema(&index("readings_of_workflow", "readings", &["of_workflow"]));
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(text("reading"))
            .take(),
    );
}
