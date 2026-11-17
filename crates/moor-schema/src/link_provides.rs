//! Keep how each machine's agent provides Boxes itself, for `moor node ls`.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the nullable `machine_links.provides` column. A link from before has none, which reads as
/// an agent that provides no Boxes itself.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("machine_links")
            .add_column(text("provides"))
            .take(),
    );
}
