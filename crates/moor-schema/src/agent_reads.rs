//! Keep when the controller last read each launch's agent, and when the status it last reported
//! as needing input was set, in place of the hook reports it cached: the agent reports its status
//! to its terminal now.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Drop `hook_cache` and add the launches' `agent_read_at` and `asked_at` columns.
pub(super) fn write(s: &mut Script) {
    s.schema(&Table::drop().table("hook_cache").take());
    s.schema(
        &Table::alter()
            .table("launches")
            .add_column(int("agent_read_at"))
            .take(),
    );
    s.schema(
        &Table::alter()
            .table("launches")
            .add_column(int("asked_at"))
            .take(),
    );
}
