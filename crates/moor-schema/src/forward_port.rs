//! Give each `proxy` row the loopback port the Box's egress forwarder listens on. A row from
//! before it gets one at its next reserve.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `forward_port` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(int("forward_port"))
            .take(),
    );
}
