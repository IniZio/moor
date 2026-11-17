//! Keep whether a Box's last suspend let its provider save its caches. Null means it did not.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `suspend_publish` column. Nothing reads or writes it since caches stopped being saved
/// at pause. It stays for a controller one release older, and a later migration drops it.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("boxes")
            .add_column(int("suspend_publish"))
            .take(),
    );
}
