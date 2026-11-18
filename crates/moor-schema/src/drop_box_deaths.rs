//! Drop `box_state.recent_deaths`: a Box that dies is left stuck at its first death, so nothing
//! counts deaths.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

/// Drop `box_state.recent_deaths`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("box_state")
            .drop_column("recent_deaths")
            .take(),
    );
}
