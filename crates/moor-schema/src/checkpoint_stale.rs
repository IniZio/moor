//! Keep the oldest commit each setup checkpoint's ignored files may come from, and which
//! checkpoint each Box stands on, so a retired one goes only once no Box stands on it. A row from
//! before has neither. moor reads a missing commit as the base for a row set up from scratch, and
//! as unknown for one that rolled forward, and a pin of no checkpoint holds every row of its layer.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the checkpoints' `stale_since` column and the pins' `checkpoint` column.
pub(super) fn write(s: &mut Script) {
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
