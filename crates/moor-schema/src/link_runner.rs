//! Record which runner holds each machine's link, when it last reported, and since when it
//! drains, so the controller routes a machine's ops to the runner that holds it. A null
//! `runner_addr` is a link no runner has reported since this step, which reads as offline.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{int, text};

/// Add the `runner_addr`, `heard_at` and `draining_since` columns.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("machine_links")
            .add_column(text("runner_addr"))
            .take(),
    );
    for column in ["heard_at", "draining_since"] {
        s.schema(
            &Table::alter()
                .table("machine_links")
                .add_column(int(column))
                .take(),
        );
    }
}
