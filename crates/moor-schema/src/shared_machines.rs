//! Let a machine have no owner, so a machine agent the operator enrolls on the controller's own
//! host is a shared node. `user_machines_v2.user` becomes nullable, and a partial unique index
//! keeps one shared machine per name, which `(user, name)` does not, as it ignores null.
//!
//! Postgres drops the constraint in place. `SQLite` cannot, so it rebuilds the table. Three
//! tables cascade from `user_machines_v2`, so dropping it would delete their rows with
//! `foreign_keys` on; their rows are set aside in temp tables and put back. Every machine keeps
//! its `machine_id`, which its agent holds in its state.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::{Backend, Script};

use crate::{fk, int, key, text};

/// The tables with a foreign key to `user_machines_v2` that cascades on delete.
const CHILDREN: [&str; 3] = ["machine_links", "machine_boxes", "machine_box_memory"];

/// The columns of `user_machines_v2`, in table order.
const COLUMNS: &str =
    "machine_id, \"user\", name, pubkey, labels, capacity, enrolled_at, pool_provider";

/// Make `user_machines_v2.user` nullable and add the index over shared machines' names.
pub(super) fn write(s: &mut Script) {
    match s.backend() {
        Backend::Postgres => {
            s.raw("ALTER TABLE \"user_machines_v2\" ALTER COLUMN \"user\" DROP NOT NULL");
        }
        Backend::Sqlite => rebuild(s),
    }
    s.raw(
        "CREATE UNIQUE INDEX \"user_machines_v2_shared_name\" ON \"user_machines_v2\" (\"name\") \
         WHERE \"user\" IS NULL",
    );
}

/// Rebuild `user_machines_v2` with a nullable `user`, keeping its children's rows.
fn rebuild(s: &mut Script) {
    for child in CHILDREN {
        s.raw(&format!(
            "CREATE TEMP TABLE \"{child}_keep\" AS SELECT * FROM \"{child}\""
        ));
    }
    s.schema(
        Table::create()
            .table("user_machines_v2_new")
            .col(int("machine_id").primary_key())
            .col(int("user"))
            .col(text("name").not_null())
            .col(text("pubkey").not_null())
            .col(text("labels").not_null().default("[]"))
            .col(
                int("capacity")
                    .not_null()
                    .check(Expr::col("capacity").gte(1)),
            )
            .col(int("enrolled_at").not_null())
            .col(text("pool_provider"))
            .index(key(&["user", "name"]).unique())
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.raw(&format!(
        "INSERT INTO \"user_machines_v2_new\" ({COLUMNS}) SELECT {COLUMNS} FROM \"user_machines_v2\""
    ));
    s.raw("DROP TABLE \"user_machines_v2\"");
    s.raw("ALTER TABLE \"user_machines_v2_new\" RENAME TO \"user_machines_v2\"");
    // The drop cascaded with foreign keys on, and did not with them off; either way start empty.
    for child in CHILDREN {
        s.raw(&format!("DELETE FROM \"{child}\""));
        s.raw(&format!(
            "INSERT INTO \"{child}\" SELECT * FROM \"{child}_keep\""
        ));
        s.raw(&format!("DROP TABLE \"{child}_keep\""));
    }
}
