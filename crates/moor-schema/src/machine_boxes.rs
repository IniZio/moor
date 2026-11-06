//! Keep the runner's record of each Box it set up on a machine in the store, so any runner can
//! wake a Box another prepared and release a closed one. `reach` and `facts` are the runner's
//! JSON, which the controller never reads.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::{fk_cascade, int, text};

/// Create `machine_boxes`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("machine_boxes")
            .col(int("box_id").primary_key())
            .col(int("machine_id").not_null())
            .col(int("slot").not_null())
            .col(text("reach").not_null())
            .col(text("facts").not_null())
            .col(int("prepared").not_null())
            .col(int("suspended_at"))
            .col(int("evicted").not_null())
            .col(int("updated_at").not_null())
            .foreign_key(&mut fk_cascade(
                "machine_id",
                "user_machines_v2",
                "machine_id",
            )),
    );
}
