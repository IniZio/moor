//! Drop the Workspace and pane columns: the agent runs in its own terminal on the Box, and the
//! controller keeps no Workspace or pane for it.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

/// Drop `sessions.workspace_id`, `sessions.pane` and `launches.pane`.
pub(super) fn write(s: &mut Script) {
    for (table, column) in [
        ("sessions", "workspace_id"),
        ("sessions", "pane"),
        ("launches", "pane"),
    ] {
        s.schema(&Table::alter().table(table).drop_column(column).take());
    }
}
