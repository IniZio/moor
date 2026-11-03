//! Enrolled machines and their links, Box claims on them, node pressure, and setup checkpoints.

use moor_migrate::sea_query::{ConditionalStatement, Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::{fk, fk_cascade, index, int, key, row_id, text};

/// The machine and checkpoint tables.
pub(super) fn write(s: &mut Script) {
    for table in [
        user_machines_v2,
        machine_links,
        box_machines,
        node_pressure,
        machine_box_memory,
        setup_checkpoints,
        setup_checkpoint_pins,
    ] {
        table(s);
    }
}

/// The `user_machines_v2` table.
fn user_machines_v2(s: &mut Script) {
    let id = row_id(s, "machine_id");
    s.schema(
        Table::create()
            .table("user_machines_v2")
            .col(id)
            .col(int("user").not_null())
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
}

/// The `machine_links` table.
fn machine_links(s: &mut Script) {
    s.schema(
        Table::create()
            .table("machine_links")
            .col(int("machine_id").primary_key())
            .col(int("online").not_null())
            .col(int("since").not_null())
            .col(text("version"))
            .col(int("protocol"))
            .col(int("max_boxes"))
            .col(int("cache_gb"))
            .col(text("pool_features").not_null().default("[]"))
            .col(int("mem_mib"))
            .col(int("cpus"))
            .col(int("reserve_mib"))
            .col(int("mem_available_mib"))
            .col(int("psi_centi"))
            .col(int("reported_at"))
            .col(int("psi_full_centi"))
            .col(int("disk_free_mib"))
            .col(int("disk_total_mib"))
            .foreign_key(&mut fk_cascade(
                "machine_id",
                "user_machines_v2",
                "machine_id",
            )),
    );
}

/// The `node_pressure` table.
fn node_pressure(s: &mut Script) {
    s.schema(
        Table::create()
            .table("node_pressure")
            .col(text("node").primary_key())
            .col(text("readings").not_null())
            .col(int("high_since"))
            .col(int("suspended_at")),
    );
}

/// The `machine_box_memory` table.
fn machine_box_memory(s: &mut Script) {
    s.schema(
        Table::create()
            .table("machine_box_memory")
            .col(int("machine_id").not_null())
            .col(int("box_id").not_null())
            .col(int("host_memory_mib").not_null())
            .primary_key(&mut key(&["machine_id", "box_id"]))
            .foreign_key(&mut fk_cascade(
                "machine_id",
                "user_machines_v2",
                "machine_id",
            )),
    );
}

/// The `setup_checkpoints` table.
fn setup_checkpoints(s: &mut Script) {
    s.schema(
        Table::create()
            .table("setup_checkpoints")
            .col(text("provider").not_null())
            .col(text("layer_key").not_null())
            .col(text("checkpoint").not_null())
            .col(text("repo").not_null())
            .col(int("bytes"))
            .col(int("created_at").not_null())
            .col(int("last_used_at").not_null())
            .col(text("workdir"))
            .col(int("setup_ms"))
            .col(int("user"))
            .col(text("chain"))
            .col(int("chain_at"))
            .col(text("parent"))
            .col(int("evicting").not_null().default(0))
            .primary_key(&mut key(&["provider", "checkpoint"])),
    );
    s.schema(
        index(
            "setup_checkpoints_layer",
            "setup_checkpoints",
            &["provider", "layer_key"],
        )
        .unique()
        .and_where(Expr::col("evicting").eq(0)),
    );
    s.schema(&index(
        "setup_checkpoints_parent",
        "setup_checkpoints",
        &["provider", "parent"],
    ));
}

/// The `setup_checkpoint_pins` table.
fn setup_checkpoint_pins(s: &mut Script) {
    s.schema(
        Table::create()
            .table("setup_checkpoint_pins")
            .col(int("box_id").not_null())
            .col(text("provider").not_null())
            .col(text("layer_key").not_null())
            .col(int("layer").not_null())
            .primary_key(&mut key(&["box_id", "provider", "layer_key"])),
    );
}

/// A Box's claim on a machine: wanted, held on a slot, or asleep off it.
fn box_machines(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_machines")
            .col(int("box").primary_key())
            .col(int("user").not_null())
            .col(
                text("state")
                    .not_null()
                    .check(Expr::col("state").is_in(["wanted", "held", "asleep"])),
            )
            .col(text("machine"))
            .col(text("labels").not_null().default("[]"))
            .col(text("ports").not_null())
            .col(int("slot").not_null().default(0))
            .col(text("sync_exclude").not_null().default("[]"))
            .col(int("since").not_null().default(0))
            .col(int("until"))
            .col(int("enrolled").not_null().default(0))
            .check(
                Expr::col("state")
                    .eq("wanted")
                    .or(Expr::col("machine").is_not_null()),
            )
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.schema(
        index(
            "box_machines_slot",
            "box_machines",
            &["user", "machine", "slot"],
        )
        .unique()
        .and_where(Expr::col("state").eq("held")),
    );
    s.schema(
        index("box_machines_wanted", "box_machines", &["user", "since"])
            .and_where(Expr::col("state").eq("wanted")),
    );
}
