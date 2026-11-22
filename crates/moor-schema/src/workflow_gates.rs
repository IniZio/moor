//! Let a workflow carry gates in place of a plan. A workflow from before has none, so its runs walk
//! its plan or the single prompt as they did.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the nullable `gates` column: the gates as JSON, or null for none.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(text("gates"))
            .take(),
    );
}
