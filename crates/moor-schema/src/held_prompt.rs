//! Keep a prompt given to a start that had to queue for an asleep Box until its agent has it.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the `held_prompt` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(text("held_prompt"))
            .take(),
    );
}
