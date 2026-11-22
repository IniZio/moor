//! Keep the outcome a run of a Box with gates ended in. A run from before has none, as does every
//! run without gates.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the nullable `workflow_runs.verdict`: the outcome as JSON, or null for none.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflow_runs")
            .add_column(text("verdict"))
            .take(),
    );
}
