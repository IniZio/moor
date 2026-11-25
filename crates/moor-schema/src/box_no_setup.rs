//! Keep whether a Box was started with no setup, so every later machine it gets skips setup too.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the boxes' `no_setup`: 1 when the start asked to skip the repo's setup lines, else 0, as
/// every Box was before this step.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(int("no_setup").not_null().default(0).take())
            .take(),
    );
}
