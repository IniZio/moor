//! Keep each machine node's settings on its row, edited in moor rather than set by its agent.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add `user_machines_v2.max_boxes`, the most live Boxes the node holds, none for no cap; the
/// `settings_revision` that counts its settings' writes from 0; and who wrote them last and when.
/// A node from before has no cap and no write, so it behaves as it did: its agent's old offer of
/// a cap was never kept as one.
pub(super) fn write(s: &mut Script) {
    for column in [
        int("max_boxes"),
        int("settings_revision").not_null().default(0).take(),
        text("settings_edited_by"),
        int("settings_edited_at"),
    ] {
        s.schema(
            &Table::alter()
                .table("user_machines_v2")
                .add_column(column)
                .take(),
        );
    }
}
