//! Fold each workflow's `also` into its `repos`, after its one repo, now that a workflow's repos
//! are the ordered set one Box clones.
//!
//! A workflow with more than one repo started one Box per repo, and would now start one Box with
//! them all, so the step refuses such a store instead of changing what it runs. It inserts each
//! workflow's repo count into a table whose check allows at most one, and the check names why.
//! `also` is left in place, emptied, so the step stays a data patch.

use moor_migrate::{Backend, Script};

/// Refuse a workflow of several repos, then fold `also` into `repos`.
pub(super) fn write(s: &mut Script) {
    let count = match s.backend() {
        Backend::Sqlite => "json_array_length(\"repos\")",
        Backend::Postgres => "json_array_length(\"repos\"::json)",
    };
    s.raw(
        "CREATE TABLE \"workflow_repo_set_guard\" (\"n\" bigint NOT NULL, \
         CONSTRAINT \"a_stored_workflow_has_more_than_one_repo\" CHECK (\"n\" <= 1))",
    );
    s.raw(&format!(
        "INSERT INTO \"workflow_repo_set_guard\" (\"n\") SELECT {count} FROM \"workflows\""
    ));
    s.raw("DROP TABLE \"workflow_repo_set_guard\"");
    s.raw(
        "UPDATE \"workflows\" SET \"repos\" = substr(\"repos\", 1, length(\"repos\") - 1) || ',' \
         || substr(\"also\", 2), \"also\" = '[]' WHERE \"also\" <> '[]'",
    );
}
