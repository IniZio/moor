//! Give each `proxy` row the hash of the key its supervisor presents on the ledger socket and
//! the ledger protocol that supervisor was started to speak. Rows from before the ledger have
//! neither, so the next controller start replaces their supervisors.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `key_hash` and `ledger` columns.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(text("key_hash"))
            .take(),
    );
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(int("ledger"))
            .take(),
    );
}
