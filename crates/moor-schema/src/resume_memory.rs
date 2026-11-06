//! Keep what each Box's suspended VM needs to resume, as its provider reports it, so a resume is
//! gated on that rather than on the guest's whole memory. A Box left null falls back to the latter.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Add the `resume_memory_mib` column.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("machine_box_memory")
            .add_column(int("resume_memory_mib"))
            .take(),
    );
}
