//! Let a workflow have no cron, so it fires only by hand or on a pull request. `workflows.cron`
//! and `workflows.timezone` become nullable, both null together; every stored workflow keeps its
//! cron.
//!
//! Postgres drops the constraints in place. `SQLite` cannot, so it rebuilds the table. The
//! `workflow_runs` rows cascade from it, and `workflow_run_history` rows from those, so dropping
//! it would delete them with `foreign_keys` on; they are set aside in temp tables and put back.
//! Every workflow keeps its id, which its runs hold.

use moor_migrate::{Backend, Script};

/// The columns of `workflows`, in table order.
const COLUMNS: &str = "id, \"user\", name, cron, timezone, repos, prompt, enabled_at, \
    timeout_minutes, created_at, updated_at, origin_front, origin_key, also, config, plan, \
    revision, on_pull_request";

/// The tables whose rows cascade from `workflows`, parents first.
const CHILDREN: [&str; 2] = ["workflow_runs", "workflow_run_history"];

/// Make `workflows.cron` and `workflows.timezone` nullable.
pub(super) fn write(s: &mut Script) {
    match s.backend() {
        Backend::Postgres => {
            for column in ["cron", "timezone"] {
                s.raw(&format!(
                    "ALTER TABLE \"workflows\" ALTER COLUMN \"{column}\" DROP NOT NULL"
                ));
            }
        }
        Backend::Sqlite => rebuild(s),
    }
}

/// Rebuild `workflows` with nullable `cron` and `timezone`, keeping its children's rows.
fn rebuild(s: &mut Script) {
    for child in CHILDREN {
        s.raw(&format!(
            "CREATE TEMP TABLE \"{child}_keep\" AS SELECT * FROM \"{child}\""
        ));
    }
    s.raw(
        "CREATE TABLE \"workflows_new\" ( \
         \"id\" integer PRIMARY KEY AUTOINCREMENT, \"user\" integer NOT NULL, \
         \"name\" text NOT NULL, \"cron\" text, \"timezone\" text, \"repos\" text NOT NULL, \
         \"prompt\" text NOT NULL, \"enabled_at\" integer, \"timeout_minutes\" integer NOT NULL, \
         \"created_at\" integer NOT NULL, \"updated_at\" integer NOT NULL, \
         \"origin_front\" text, \"origin_key\" text, \"also\" text NOT NULL DEFAULT '[]', \
         \"config\" text, \"plan\" text, \"revision\" integer NOT NULL DEFAULT 0, \
         \"on_pull_request\" integer NOT NULL DEFAULT 0, \
         UNIQUE (\"user\", \"name\"), \
         FOREIGN KEY (\"user\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE)",
    );
    s.raw(&format!(
        "INSERT INTO \"workflows_new\" ({COLUMNS}) SELECT {COLUMNS} FROM \"workflows\""
    ));
    s.raw("DROP TABLE \"workflows\"");
    s.raw("ALTER TABLE \"workflows_new\" RENAME TO \"workflows\"");
    // The drop cascaded with foreign keys on, and did not with them off; either way start empty.
    for child in CHILDREN {
        s.raw(&format!("DELETE FROM \"{child}\""));
        s.raw(&format!(
            "INSERT INTO \"{child}\" SELECT * FROM \"{child}_keep\""
        ));
        s.raw(&format!("DROP TABLE \"{child}_keep\""));
    }
}
