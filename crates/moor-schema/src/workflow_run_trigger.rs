//! Record what fired each workflow run, a schedule or a person.
//!
//! An old run was fired by its schedule, so the column defaults to `cron` and no row is
//! rewritten.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::text;

/// Add `workflow_runs.trigger`.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("workflow_runs")
            .add_column(
                text("trigger")
                    .not_null()
                    .default("cron")
                    .check(Expr::col("trigger").is_in(["cron", "manual"]))
                    .take(),
            )
            .take(),
    );
}
