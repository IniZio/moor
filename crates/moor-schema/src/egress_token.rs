//! Give each `proxy` row the hash of the token its Box presents to the shared egress Proxy.
//! Rows from before it have none, so their Boxes are refused there until they start again.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the `token_hash` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(text("token_hash"))
            .take(),
    );
}
