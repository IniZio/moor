//! Give each `proxy` row the hash of the token its Box presents to the shared egress Proxy, and
//! whether that token reached the Box. Rows from before it have none, so their Boxes are refused
//! there until they start again.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `token_hash` and `token_delivered` columns.
pub(super) fn write(s: &mut Script) {
    for col in [text("token_hash"), int("token_delivered")] {
        s.schema(&Table::alter().table("proxy").add_column(col).take());
    }
}
