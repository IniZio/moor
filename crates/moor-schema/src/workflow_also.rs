//! Give each workflow the repos its Boxes clone beside their own. A workflow from before it
//! clones none, as it did.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the `also` column, a JSON list of `OWNER/NAME`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflows")
            .add_column(text("also").not_null().default("[]").take())
            .take(),
    );
}
