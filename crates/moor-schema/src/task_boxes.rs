//! Tie a work Box to the claimed place that started it.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{index, int};

/// Add the boxes' `task`, the place whose agent started the Box, and an index to find a place's
/// Boxes by.
///
/// The column is set when the Box is born and never changed. A Box a person or a workflow
/// started has none, as every Box did before this step.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(int("task").extra("REFERENCES boxes (id)"))
            .take(),
    );
    s.schema(&index("boxes_task", "boxes", &["task"]));
}
