//! Give a workflow a plan and a revision, and a run a step pointer, a phase and a history.
//!
//! The `outcome` column's check grows from six values to ten. Postgres replaces the check in
//! place. `SQLite` cannot, so it rebuilds `workflow_runs` the way `V80__shared_machines` rebuilds
//! `user_machines_v2`; no table has a foreign key to `workflow_runs` yet, so the rebuild is copy,
//! drop and rename, and the history table with its foreign key is made after it. Every old
//! `outcome` keeps its meaning, so no row is rewritten.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::{Backend, Script};

use crate::{fk_cascade, index, int, key, row_id, text};

/// Every value `workflow_runs.outcome` may hold.
const OUTCOMES: [&str; 10] = [
    "starting",
    "started",
    "waiting",
    "finished",
    "failed",
    "incomplete",
    "declined",
    "timed_out",
    "skipped_overlap",
    "refused",
];

/// The columns of `workflow_runs` before this step, in table order.
const OLD_COLUMNS: &str = "id, workflow, repo, due_at, box, outcome, detail, ended_at, \
                           recorded_at, seq";

/// Add the plan columns, widen the outcomes, and add the history table and the Box index.
pub(super) fn write(s: &mut Script) {
    for column in [text("plan"), int("revision").not_null().default(0).take()] {
        s.schema(&Table::alter().table("workflows").add_column(column).take());
    }
    match s.backend() {
        Backend::Postgres => widen_postgres(s),
        Backend::Sqlite => rebuild(s),
    }
    s.schema(&index("workflow_runs_box", "workflow_runs", &["box"]));
    s.schema(
        Table::create()
            .table("workflow_run_history")
            .col(int("run").not_null())
            .col(int("n").not_null())
            .col(int("at").not_null())
            .col(text("entry").not_null())
            .index(key(&["run", "n"]).unique())
            .foreign_key(&mut fk_cascade("run", "workflow_runs", "id")),
    );
}

/// Add the run columns and replace the `outcome` check.
fn widen_postgres(s: &mut Script) {
    for column in [
        text("plan"),
        text("step"),
        int("try_n"),
        int("waiting_since"),
        text("ended_step"),
    ] {
        s.schema(
            &Table::alter()
                .table("workflow_runs")
                .add_column(column)
                .take(),
        );
    }
    s.raw("ALTER TABLE \"workflow_runs\" DROP CONSTRAINT \"workflow_runs_outcome_check\"");
    let values = OUTCOMES.map(|o| format!("'{o}'")).join(", ");
    s.raw(&format!(
        "ALTER TABLE \"workflow_runs\" ADD CONSTRAINT \"workflow_runs_outcome_check\" \
         CHECK (\"outcome\" IN ({values}))"
    ));
}

/// Rebuild `workflow_runs` with the new columns and the wider check, keeping every row and id.
fn rebuild(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("workflow_runs_new")
            .col(id)
            .col(int("workflow").not_null())
            .col(text("repo").not_null())
            .col(int("due_at").not_null())
            .col(int("box"))
            .col(
                text("outcome")
                    .not_null()
                    .check(Expr::col("outcome").is_in(OUTCOMES)),
            )
            .col(text("detail"))
            .col(int("ended_at"))
            .col(int("recorded_at").not_null())
            .col(int("seq").not_null().default(0))
            .col(text("plan"))
            .col(text("step"))
            .col(int("try_n"))
            .col(int("waiting_since"))
            .col(text("ended_step"))
            .index(key(&["workflow", "repo", "due_at"]).unique())
            .foreign_key(&mut fk_cascade("workflow", "workflows", "id")),
    );
    s.raw(&format!(
        "INSERT INTO \"workflow_runs_new\" ({OLD_COLUMNS}) SELECT {OLD_COLUMNS} \
         FROM \"workflow_runs\""
    ));
    s.raw("DROP TABLE \"workflow_runs\"");
    s.raw("ALTER TABLE \"workflow_runs_new\" RENAME TO \"workflow_runs\"");
    s.schema(&index(
        "workflow_runs_workflow",
        "workflow_runs",
        &["workflow"],
    ));
}
