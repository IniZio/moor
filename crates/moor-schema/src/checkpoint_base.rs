//! Keep the commit each setup checkpoint's disk holds, which is no longer part of its key. A row
//! from before has none, so no Box starts from it, and eviction removes it in time.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the `base` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("setup_checkpoints")
            .add_column(text("base"))
            .take(),
    );
}
