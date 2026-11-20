//! Let a workflow fire on its first repo's pull request events. A workflow from before it fires
//! on its cron and by hand only, as it did.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `on_pull_request` column, 1 for on and 0 for off.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(int("on_pull_request").not_null().default(0).take())
            .take(),
    );
}
