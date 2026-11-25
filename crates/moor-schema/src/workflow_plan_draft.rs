//! Keep where moor stands in making a workflow's plan from its prompt. A workflow from before it
//! has none, so its stored plan stays until its prompt changes or its owner asks for a new one.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the workflows' `plan_draft`: JSON of the plan moor is making or failed to make; null for
/// none.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(text("plan_draft"))
            .take(),
    );
}
