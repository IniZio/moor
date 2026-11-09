//! Keep a Box's VM in its own row, `box_hands`, so the Box can outlive the VM. A Box's current
//! VM is its row with the highest `inc`; every Box that had a VM starts at `inc` 1.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk, index, int, key, text};

/// The VM columns of `boxes`, each with the `box_hands` column it moves to.
const MOVED: [(&str, &str); 14] = [
    ("provider", "provider"),
    ("handle", "handle"),
    ("attempted_handle", "attempted_handle"),
    ("driver_state", "state"),
    ("driver_updated_at", "state_at"),
    ("workdir", "workdir"),
    ("egress", "egress"),
    ("sleep_reason", "sleep_reason"),
    ("cpus", "cpus"),
    ("mem_mib", "mem_mib"),
    ("suspend_ahead", "suspend_ahead"),
    ("suspend_dirty", "suspend_dirty"),
    ("suspend_checked_at", "suspend_checked_at"),
    ("suspend_publish", "suspend_publish"),
];

/// Create `box_hands`, copy each Box's VM into it and drop the VM columns from `boxes`, with
/// `node_mem_mib`, which nothing reads.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_hands")
            .col(int("box").not_null())
            .col(int("inc").not_null())
            .col(text("provider").not_null())
            .col(text("handle"))
            .col(text("attempted_handle"))
            .col(text("state").not_null())
            .col(int("state_at").not_null())
            .col(text("workdir"))
            .col(text("egress"))
            .col(text("sleep_reason"))
            .col(int("cpus"))
            .col(int("mem_mib"))
            .col(int("suspend_ahead"))
            .col(int("suspend_dirty"))
            .col(int("suspend_checked_at"))
            .col(int("suspend_publish"))
            .col(int("created_at").not_null())
            .primary_key(&mut key(&["box", "inc"]))
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
    s.schema(&index("box_hands_state", "box_hands", &["state"]));
    let quoted = |cols: &mut dyn Iterator<Item = &str>| {
        cols.map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // The store kept no time the VM was made, so its last state change stands in.
    s.raw(&format!(
        "INSERT INTO \"box_hands\" (\"box\", \"inc\", {}, \"created_at\") \
         SELECT \"id\", 1, {}, \"driver_updated_at\" FROM \"boxes\" WHERE \"provider\" IS NOT NULL",
        quoted(&mut MOVED.iter().map(|(_, to)| *to)),
        quoted(&mut MOVED.iter().map(|(from, _)| *from)),
    ));
    for column in MOVED
        .map(|(from, _)| from)
        .into_iter()
        .chain(["node_mem_mib"])
    {
        s.schema(&Table::alter().table("boxes").drop_column(column).take());
    }
}
