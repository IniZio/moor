//! Drop `workflows.disabled_reason` and `workflows.change_seq`: nothing disables a workflow but its
//! owner now, so a disabling has no reason and takes no place in the change stream.
//!
//! A workflow the old breaker disabled would stay off with no word why, so it is enabled again, as
//! of when it was disabled. The one overdue firing it may have fires once, as after any outage.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

/// Enable the workflows the breaker disabled, then drop the two columns.
pub(super) fn write(s: &mut Script) {
    s.raw(
        "UPDATE \"workflows\" SET \"enabled_at\" = \"updated_at\" \
         WHERE \"enabled_at\" IS NULL AND \"disabled_reason\" IS NOT NULL",
    );
    for column in ["disabled_reason", "change_seq"] {
        s.schema(&Table::alter().table("workflows").drop_column(column).take());
    }
}
