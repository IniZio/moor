//! Keep each Proxy's spec and interception CA in its `proxy` row, so any process that reaches the
//! controller can start the Proxy. The CA key is sealed with the vault.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{blob, text};

/// Add the `spec`, `ca_cert` and `ca_key` columns.
pub(super) fn write(s: &mut Script) {
    for col in [text("spec"), text("ca_cert"), blob("ca_key")] {
        s.schema(&Table::alter().table("proxy").add_column(col).take());
    }
}
