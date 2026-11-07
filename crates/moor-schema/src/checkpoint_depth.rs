//! Keep how many times each setup checkpoint's chain rolled forward, which bounds its deltas, the
//! oldest commit its ignored files may come from, and which checkpoint each Box stands on, so a
//! retired one goes only once no Box stands on it. A row from before has a depth of 0, its own
//! base as that commit, and a pin of no checkpoint, which holds every row of its layer.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `depth` and `stale_since` columns and the pins' `checkpoint` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("setup_checkpoints")
            .add_column(int("depth"))
            .take(),
    );
    s.schema(
        &Table::alter()
            .table("setup_checkpoints")
            .add_column(text("stale_since"))
            .take(),
    );
    s.schema(
        &Table::alter()
            .table("setup_checkpoint_pins")
            .add_column(text("checkpoint"))
            .take(),
    );
}
