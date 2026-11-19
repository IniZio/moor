//! Keep the ref a person started a Box from, beside the commit it resolved to.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the boxes' `base_ref` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(text("base_ref"))
            .take(),
    );
}
