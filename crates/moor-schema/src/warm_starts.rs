//! Keep whether each machine's agent serves checkpoints, and why not, for placing a Box and for
//! `moor node ls`.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the `warm_starts` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("machine_links")
            .add_column(text("warm_starts"))
            .take(),
    );
}
