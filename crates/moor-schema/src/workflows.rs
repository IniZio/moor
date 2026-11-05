//! Workflows: each user's recurring Box starts, and the record of each firing so a restart never
//! fires one twice.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::{fk_cascade, index, int, key, row_id, serial_id, text};

/// Add the `workflows` and `workflow_runs` tables.
pub(super) fn write(s: &mut Script) {
    s.schema(
        Table::create()
            .table("workflows")
            .col(serial_id("id"))
            .col(int("user").not_null())
            .col(text("name").not_null())
            .col(text("cron").not_null())
            .col(text("timezone").not_null())
            .col(text("repos").not_null())
            .col(text("prompt").not_null())
            .col(int("enabled_at"))
            .col(text("disabled_reason"))
            .col(int("timeout_minutes").not_null())
            .col(int("created_at").not_null())
            .col(int("updated_at").not_null())
            .index(key(&["user", "name"]).unique())
            .foreign_key(&mut fk_cascade("user", "users", "id")),
    );
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("workflow_runs")
            .col(id)
            .col(int("workflow").not_null())
            .col(text("repo").not_null())
            .col(int("due_at").not_null())
            .col(int("box"))
            .col(
                text("outcome")
                    .not_null()
                    .check(Expr::col("outcome").is_in([
                        "starting",
                        "started",
                        "finished",
                        "timed_out",
                        "skipped_overlap",
                        "refused",
                    ])),
            )
            .col(text("detail"))
            .col(int("ended_at"))
            .col(int("recorded_at").not_null())
            .index(key(&["workflow", "repo", "due_at"]).unique())
            .foreign_key(&mut fk_cascade("workflow", "workflows", "id")),
    );
    s.schema(&index(
        "workflow_runs_workflow",
        "workflow_runs",
        &["workflow"],
    ));
}
