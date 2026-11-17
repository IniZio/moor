//! Drop the setup checkpoint tables: a Box starts cold, so nothing saves or pins a checkpoint.

use moor_migrate::sea_query::Table;
use moor_migrate::Script;

/// Drop `setup_checkpoint_pins`, then `setup_checkpoints`.
pub(super) fn write(s: &mut Script) {
    s.schema(&Table::drop().table("setup_checkpoint_pins").take());
    s.schema(&Table::drop().table("setup_checkpoints").take());
}
