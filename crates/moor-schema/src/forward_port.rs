//! Give each `proxy` row the loopback port the Box's egress forwarder listens on, unique among
//! rows that have one. A row from before it gets one at its next reserve.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{index, int};

/// Add the `forward_port` column and its unique index; rows without a port do not collide.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(int("forward_port"))
            .take(),
    );
    s.schema(
        &index("proxy_forward_port", "proxy", &["forward_port"])
            .unique()
            .take(),
    );
}
