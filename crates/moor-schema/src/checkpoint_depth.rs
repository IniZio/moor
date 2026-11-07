//! Keep how many times each setup checkpoint's chain rolled forward, which bounds its deltas. A row
//! from before has none, which reads as 0.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `depth` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("setup_checkpoints")
            .add_column(int("depth"))
            .take(),
    );
}
