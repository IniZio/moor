//! Give a workflow run the pull request it is for: `pull_number` and `head_sha`, a
//! `pull_request` trigger, the `skipped_fork` outcome, and a key on pull request and head.
//!
//! A check ties the trigger to the two columns. A cron run has neither, a pull request run has
//! both, and a manual run has both or neither. Only a pull request run may be `skipped_fork`. A
//! partial unique index keeps one run per workflow, repo, pull request and head, so an event
//! delivered twice records once. A `skipped_fork` row sits outside it, so Run again on that row
//! can later fire the same head.
//!
//! Postgres replaces the checks in place. `SQLite` cannot, so it rebuilds `workflow_runs` as
//! `V85__workflow_plans` did. `workflow_run_history` cascades from it now, so its rows are set
//! aside in a temp table and put back, as `V80__shared_machines` does. Every old run keeps its
//! id and meaning, so no row is rewritten.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::{Backend, Script};

use crate::{fk_cascade, index, int, key, row_id, text};

/// Every value `workflow_runs.outcome` may hold.
const OUTCOMES: [&str; 11] = [
    "starting",
    "started",
    "waiting",
    "finished",
    "failed",
    "incomplete",
    "declined",
    "timed_out",
    "skipped_overlap",
    "skipped_fork",
    "refused",
];

/// Every value `workflow_runs.trigger` may hold.
const TRIGGERS: [&str; 3] = ["cron", "manual", "pull_request"];

/// The check that ties the trigger and the outcome to the pull request columns.
const SUBJECT: &str = "((\"trigger\" = 'cron' AND \"pull_number\" IS NULL AND \"head_sha\" IS NULL) \
     OR (\"trigger\" = 'manual' AND (\"pull_number\" IS NULL) = (\"head_sha\" IS NULL)) \
     OR (\"trigger\" = 'pull_request' AND \"pull_number\" IS NOT NULL AND \"head_sha\" IS NOT NULL)) \
     AND (\"outcome\" <> 'skipped_fork' OR \"trigger\" = 'pull_request')";

/// The columns of `workflow_runs` before this step, in table order.
const OLD_COLUMNS: &str = "id, workflow, repo, due_at, box, outcome, detail, ended_at, \
                           recorded_at, seq, plan, step, try_n, waiting_since, ended_step, trigger";

/// Add the pull request columns, widen the checks and add the pull request key.
pub(super) fn write(s: &mut Script) {
    match s.backend() {
        Backend::Postgres => widen_postgres(s),
        Backend::Sqlite => rebuild(s),
    }
    s.raw(
        "CREATE UNIQUE INDEX \"workflow_runs_pull\" ON \"workflow_runs\" \
         (\"workflow\", \"repo\", \"pull_number\", \"head_sha\") \
         WHERE \"trigger\" = 'pull_request' AND \"outcome\" <> 'skipped_fork'",
    );
}

/// The SQL list of `values`, quoted.
fn listed(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| format!("'{v}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Add the columns and replace the `outcome` and `trigger` checks.
fn widen_postgres(s: &mut Script) {
    for column in [int("pull_number"), text("head_sha")] {
        s.schema(
            &Table::alter()
                .table("workflow_runs")
                .add_column(column)
                .take(),
        );
    }
    for (column, values) in [("outcome", &OUTCOMES[..]), ("trigger", &TRIGGERS[..])] {
        s.raw(&format!(
            "ALTER TABLE \"workflow_runs\" DROP CONSTRAINT \"workflow_runs_{column}_check\""
        ));
        s.raw(&format!(
            "ALTER TABLE \"workflow_runs\" ADD CONSTRAINT \"workflow_runs_{column}_check\" \
             CHECK (\"{column}\" IN ({}))",
            listed(values)
        ));
    }
    s.raw(&format!(
        "ALTER TABLE \"workflow_runs\" ADD CONSTRAINT \"workflow_runs_subject_check\" \
         CHECK ({SUBJECT})"
    ));
}

/// Rebuild `workflow_runs` with the new columns and checks, keeping every row, id and history
/// line.
fn rebuild(s: &mut Script) {
    s.raw(
        "CREATE TEMP TABLE \"workflow_run_history_keep\" AS SELECT * FROM \"workflow_run_history\"",
    );
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
            .col(
                text("trigger")
                    .not_null()
                    .default("cron")
                    .check(Expr::col("trigger").is_in(TRIGGERS)),
            )
            .col(int("pull_number"))
            .col(text("head_sha"))
            .check(Expr::cust(SUBJECT))
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
    s.schema(&index("workflow_runs_box", "workflow_runs", &["box"]));
    // The drop cascaded with foreign keys on, and did not with them off; either way start empty.
    s.raw("DELETE FROM \"workflow_run_history\"");
    s.raw("INSERT INTO \"workflow_run_history\" SELECT * FROM \"workflow_run_history_keep\"");
    s.raw("DROP TABLE \"workflow_run_history_keep\"");
}
