//! Give `bindings` and `box_refs` an `id` row id, so their readers order by insertion on both
//! backends. Postgres tables have no `rowid` to order by.
//!
//! Each table is rebuilt under a new name and renamed back. On `SQLite` `id` is the rowid alias
//! and each row keeps its old rowid as its `id`, so `ORDER BY id` returns the order `ORDER BY
//! rowid` did.

use moor_migrate::sea_query::{Expr, ExprTrait, Table, TableCreateStatement};
use moor_migrate::{Backend, Script};

use crate::{fk, index, int, key, row_id, text};

/// Rebuild both tables with an `id` row id.
pub(super) fn write(s: &mut Script) {
    let id = row_id(s, "id");
    let bindings = Table::create()
        .table("bindings_new")
        .col(id.clone())
        .col(text("front").not_null())
        .col(text("conv_key").not_null())
        .col(int("session").not_null())
        .col(text("owner").not_null())
        .col(int("created_at").not_null())
        .index(key(&["front", "conv_key", "session"]).unique())
        .foreign_key(&mut fk("session", "sessions", "id"))
        .take();
    rebuild(
        s,
        "bindings",
        &bindings,
        "front, conv_key, session, owner, created_at",
        "created_at",
    );
    s.schema(&index("bindings_session", "bindings", &["session"]));
    let box_refs = Table::create()
        .table("box_refs_new")
        .col(id)
        .col(int("box").not_null())
        .col(
            text("kind")
                .not_null()
                .check(Expr::col("kind").is_in(["branch", "pr", "proof"])),
        )
        .col(text("value").not_null())
        .index(key(&["box", "kind", "value"]).unique())
        .foreign_key(&mut fk("box", "boxes", "id"))
        .take();
    rebuild(
        s,
        "box_refs",
        &box_refs,
        "box, kind, value",
        "box, kind, value",
    );
}

/// Replace `table` with `new`, a `<table>_new` with the same `cols` and an `id` row id. On
/// `SQLite` each row's `id` is its old rowid; Postgres numbers the rows in `pg_order`.
fn rebuild(s: &mut Script, table: &str, new: &TableCreateStatement, cols: &str, pg_order: &str) {
    s.schema(new);
    let copy = match s.backend() {
        Backend::Sqlite => {
            format!("INSERT INTO {table}_new (id, {cols}) SELECT rowid, {cols} FROM {table}")
        }
        Backend::Postgres => format!(
            "INSERT INTO {table}_new ({cols}) SELECT {cols} FROM {table} ORDER BY {pg_order}"
        ),
    };
    s.raw(&copy);
    s.raw(&format!("DROP TABLE {table}"));
    s.raw(&format!("ALTER TABLE {table}_new RENAME TO {table}"));
}
