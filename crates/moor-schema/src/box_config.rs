//! Let one Box, and one workflow's Boxes, carry a partial repo config laid over their repo's.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add `boxes.config_override` and `workflows.config`, nullable YAML text in the repo config's
/// format. A Box or workflow from before has none, and reads the repo's config as it did.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(text("config_override"))
            .take(),
    );
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(text("config"))
            .take(),
    );
}
