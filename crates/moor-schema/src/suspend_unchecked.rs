//! Keep why a sleep left no answer of what a Box holds, so a close that will not wake the Box
//! can tell its owner why it is held.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::text;

/// Add the nullable `box_hands.suspend_unchecked`: why the suspend of those hands kept no answer,
/// or null when one stands or the suspend kept no reason.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("box_hands")
            .add_column(text("suspend_unchecked"))
            .take(),
    );
}
