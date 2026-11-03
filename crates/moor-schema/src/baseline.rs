//! The schema SQL migrations 1 to 44 built, as one migration. Tables come before the tables
//! that reference them, as Postgres checks a reference when the table is made.

use moor_migrate::sea_query::{Query, Table};
use moor_migrate::Script;

use super::text;

mod boxes;
mod machines;
mod users;

/// Every table, index and the `meta` row a fresh moor.db starts with.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("meta")
            .col(text("k").primary_key())
            .col(text("v").not_null()),
    );
    users::write(s);
    boxes::write(s);
    machines::write(s);
    s.query(
        Query::insert()
            .into_table("meta")
            .columns(["k", "v"])
            .values_panic(["schema_version".into(), "16".into()]),
    );
}
