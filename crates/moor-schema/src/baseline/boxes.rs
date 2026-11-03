//! Boxes and what hangs off them: sessions, launches, events, runs, reports, artifacts, leases.

use moor_migrate::sea_query::{ConditionalStatement, Expr, ExprTrait, Func, Table};
use moor_migrate::{Backend, Script};

use crate::{fk, index, int, key, row_id, serial_id, text};

mod attached;

/// The Box tables.
pub(super) fn write(s: &mut Script) {
    boxes(s);
    sessions(s);
    attached::write(s);
}

/// One row per Box, and the open-Box name and branch each held by one Box at a time.
fn boxes(s: &mut Script) {
    s.schema(
        Table::create()
            .table("boxes")
            .col(serial_id("id"))
            .col(text("name").not_null())
            .col(text("repo"))
            .col(text("repo_config"))
            .col(text("author_name"))
            .col(text("author_email"))
            .col(int("user"))
            .col(text("issue"))
            .col(text("prompt"))
            .col(text("base"))
            .col(text("branch"))
            .col(text("state").not_null())
            .col(text("state_reason"))
            .col(text("intent_kind"))
            .col(int("intent_pid"))
            .col(int("intent_run"))
            .col(int("intent_at"))
            .col(int("setup_failed_at"))
            .col(int("idle_since"))
            .col(text("launch_failure"))
            .col(text("provider"))
            .col(text("handle"))
            .col(text("driver_state"))
            .col(text("attempted_handle"))
            .col(text("workdir"))
            .col(text("egress"))
            .col(text("sleep_reason"))
            .col(int("cpus"))
            .col(int("mem_mib"))
            .col(int("driver_updated_at"))
            .col(int("created_at").not_null())
            .col(int("updated_at").not_null())
            .col(text("node"))
            .col(text("node_labels").not_null().default("[]"))
            .col(int("node_mem_mib"))
            .col(int("resume_first_at"))
            .col(text("other_repos"))
            .col(int("steps_pending_at"))
            .foreign_key(&mut fk("user", "users", "id")),
    );
    let open = || Expr::col("state").is_not_in(["closed", "rejected"]);
    s.schema(
        index("boxes_open", "boxes", &["name"])
            .unique()
            .and_where(open()),
    );
    s.schema(
        index("boxes_open_branch", "boxes", &["branch"])
            .unique()
            .and_where(Expr::col("branch").is_not_null())
            .and_where(open()),
    );
}

/// Agent sessions, in a Box or on a host workdir.
fn sessions(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("sessions")
            .col(id)
            .col(int("box"))
            .col(int("ordinal").not_null())
            .col(text("agent"))
            .col(text("model"))
            .col(text("agent_session_id"))
            .col(text("workspace_id"))
            .col(text("pane"))
            .col(text("state").not_null())
            .col(int("started_at").not_null())
            .col(int("ended_at"))
            .col(int("parent_session"))
            .col(text("workdir"))
            .col(int("user"))
            .col(text("host_key"))
            .check(
                Expr::col("box")
                    .is_not_null()
                    .or(Expr::col("workdir").is_not_null()),
            )
            .index(key(&["box", "ordinal"]).unique())
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("parent_session", "sessions", "id"))
            .foreign_key(&mut fk("user", "users", "id")),
    );
    s.schema(&index("sessions_parent", "sessions", &["parent_session"]));
    // One live host session per user and key; a null user counts as one user. sea-query renders
    // no expression index for SQLite.
    let mut host_key = index("sessions_host_key", "sessions", &[]);
    host_key
        .col(Func::if_null(Expr::col("user"), 0))
        .col("host_key")
        .unique()
        .and_where(Expr::col("host_key").is_not_null())
        .and_where(Expr::col("ended_at").is_null());
    match s.backend() {
        Backend::Sqlite => s.raw(
            "CREATE UNIQUE INDEX \"sessions_host_key\" ON \"sessions\" \
             (IFNULL(\"user\", 0), \"host_key\") \
             WHERE \"host_key\" IS NOT NULL AND \"ended_at\" IS NULL",
        ),
        Backend::Postgres => s.schema(&host_key),
    };
}
