//! Keep a Box's lifecycle in its own row, `box_state`, so `boxes` holds only what the Box was
//! started with. A live Box holds its name and branch in `open_name` and `open_branch`, which a
//! plain unique index keeps to one live Box each; a closed or rejected Box holds neither.

use moor_migrate::sea_query::{Index, Table};
use moor_migrate::Script;

use crate::{fk, index, int, key, text};

/// The lifecycle columns of `boxes`; each keeps its name in `box_state`.
const MOVED: [&str; 15] = [
    "state",
    "state_reason",
    "node",
    "prompt",
    "held_prompt",
    "intent_kind",
    "intent_pid",
    "intent_run",
    "intent_at",
    "setup_failed_at",
    "steps_pending_at",
    "idle_since",
    "launch_failure",
    "resume_first_at",
    "updated_at",
];

/// Create `box_state`, copy each Box's lifecycle into it and drop the lifecycle columns, and the
/// partial indexes over them, from `boxes`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_state")
            .col(int("box").not_null())
            .col(text("state").not_null())
            .col(text("state_reason"))
            .col(text("open_name"))
            .col(text("open_branch"))
            .col(text("node"))
            .col(text("prompt"))
            .col(text("held_prompt"))
            .col(text("intent_kind"))
            .col(int("intent_pid"))
            .col(int("intent_run"))
            .col(int("intent_at"))
            .col(int("setup_failed_at"))
            .col(int("steps_pending_at"))
            .col(int("idle_since"))
            .col(text("launch_failure"))
            .col(int("resume_first_at"))
            .col(int("updated_at").not_null())
            .primary_key(&mut key(&["box"]))
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
    s.schema(
        &index("box_state_open_name", "box_state", &["open_name"])
            .unique()
            .take(),
    );
    s.schema(
        &index("box_state_open_branch", "box_state", &["open_branch"])
            .unique()
            .take(),
    );
    s.schema(&index("box_state_node", "box_state", &["node", "state"]));
    let cols = MOVED
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    s.raw(&format!(
        "INSERT INTO \"box_state\" (\"box\", \"open_name\", \"open_branch\", {cols}) \
         SELECT \"id\", \
         CASE WHEN \"state\" IN ('closed', 'rejected') THEN NULL ELSE \"name\" END, \
         CASE WHEN \"state\" IN ('closed', 'rejected') THEN NULL ELSE \"branch\" END, \
         {cols} FROM \"boxes\""
    ));
    // SQLite refuses to drop a column an index reads.
    for name in ["boxes_open", "boxes_open_branch"] {
        s.schema(Index::drop().name(name).table("boxes"));
    }
    for column in MOVED {
        s.schema(&Table::alter().table("boxes").drop_column(column).take());
    }
}
