//! Record on each table that holds facts about a Box's VM which of the Box's hands the facts are
//! about, so a write about hands the Box has since replaced can be told apart and refused.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

use crate::int;

/// Each table that holds VM facts, with the column naming its Box.
pub(crate) const TABLES: [(&str, &str); 5] = [
    ("proxy", "box"),
    ("machine_boxes", "box_id"),
    ("machine_box_memory", "box_id"),
    ("box_machines", "box"),
    ("setup_checkpoint_pins", "box_id"),
];

/// Add a nullable `inc` to each table and fill it with its Box's current hands, none for a Box
/// that has none.
pub(super) fn write(s: &mut Script) {
    for (table, of) in TABLES {
        s.schema(&Table::alter().table(table).add_column(int("inc")).take());
        s.raw(&format!(
            "UPDATE \"{table}\" SET \"inc\" = (SELECT MAX(\"inc\") FROM \"box_hands\" \
             WHERE \"box_hands\".\"box\" = \"{table}\".\"{of}\")"
        ));
    }
}
