//! The tables that hang off a Box or one of its sessions.

use moor_migrate::sea_query::{Expr, ExprTrait, Table};
use moor_migrate::Script;

use crate::{fk, index, int, key, row_id, serial_id, text};

/// Launches, refs, events, runs, reports, the hook cache, proxies, leases, artifacts, bindings,
/// profiles and webhook deliveries.
pub(super) fn write(s: &mut Script) {
    for table in [
        launches,
        box_refs,
        events,
        runs,
        reports,
        hook_cache,
        proxy,
        box_lease,
        artifacts,
        artifact_cursors,
        bindings,
        box_profiles,
        webhook_deliveries,
    ] {
        table(s);
    }
}

/// The `launches` table.
fn launches(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("launches")
            .col(id)
            .col(int("box").not_null())
            .col(text("session_ref").not_null())
            .col(text("pane"))
            .col(int("started_at").not_null())
            .col(int("ended_at"))
            .col(int("exit"))
            .col(text("transcript_digest"))
            .col(text("registered_agent_session"))
            .col(int("session"))
            .col(text("holder"))
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
}

/// The `box_refs` table.
fn box_refs(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_refs")
            .col(int("box").not_null())
            .col(
                text("kind")
                    .not_null()
                    .check(Expr::col("kind").is_in(["branch", "pr", "proof"])),
            )
            .col(text("value").not_null())
            .primary_key(&mut key(&["box", "kind", "value"]))
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
}

/// The `events` table.
fn events(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("events")
            .col(id)
            .col(int("box").not_null())
            .col(int("session"))
            .col(text("kind").not_null())
            .col(text("payload").not_null())
            .col(int("ts").not_null())
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
    s.schema(&index("events_box", "events", &["box", "id"]));
}

/// The `runs` table.
fn runs(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("runs")
            .col(id)
            .col(int("box").not_null())
            .col(text("argv").not_null())
            .col(text("record"))
            .col(text("sha"))
            .col(int("pid").not_null())
            .col(int("started_at").not_null())
            .col(int("finished_at"))
            .col(int("exit"))
            .col(int("interrupted").not_null().default(0)),
    );
}

/// The `reports` table.
fn reports(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("reports")
            .col(id)
            .col(int("box").not_null())
            .col(int("session"))
            .col(text("kind").not_null())
            .col(int("version").not_null())
            .col(text("path").not_null())
            .col(text("title"))
            .col(int("created_at").not_null())
            .index(key(&["box", "kind", "version"]).unique())
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
}

/// The `hook_cache` table.
fn hook_cache(s: &mut Script) {
    s.schema(
        Table::create()
            .table("hook_cache")
            .col(int("session").primary_key())
            .col(text("record").not_null())
            .col(int("at").not_null())
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
}

/// The `proxy` table.
fn proxy(s: &mut Script) {
    s.schema(
        Table::create()
            .table("proxy")
            .col(int("box").primary_key())
            .col(int("box_port").not_null().unique_key())
            .col(int("egress_port").not_null().unique_key())
            .col(int("pid"))
            .col(text("handle"))
            .col(int("ready").not_null().default(0))
            .col(text("lease_owner"))
            .col(int("lease_heartbeat"))
            .col(text("boot_id"))
            .col(int("updated_at").not_null())
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
}

/// The `box_lease` table.
fn box_lease(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_lease")
            .col(serial_id("id"))
            .col(int("box").not_null())
            .col(text("kind").not_null())
            .col(text("key").not_null())
            .col(text("state").not_null())
            .col(int("booted").not_null().default(0))
            .col(int("pgid"))
            .col(int("sid"))
            .col(int("root_pid"))
            .col(int("root_start"))
            .col(int("size_at_release"))
            .col(int("created_at").not_null())
            .col(int("updated_at").not_null())
            .index(key(&["box", "kind", "key"]).unique())
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
    s.schema(&index("box_lease_box", "box_lease", &["box"]));
}

/// The `artifacts` table.
fn artifacts(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("artifacts")
            .col(id)
            .col(int("box").not_null())
            .col(int("session"))
            .col(text("name").not_null())
            .col(int("version").not_null())
            .col(text("kind").not_null())
            .col(text("content_type").not_null())
            .col(int("size").not_null())
            .col(text("sha256").not_null())
            .col(text("path").not_null())
            .col(int("created_at").not_null())
            .index(key(&["box", "name", "version"]).unique())
            .foreign_key(&mut fk("box", "boxes", "id"))
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
}

/// The `artifact_cursors` table.
fn artifact_cursors(s: &mut Script) {
    s.schema(
        Table::create()
            .table("artifact_cursors")
            .col(int("box").primary_key())
            .col(int("through").not_null())
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
}

/// The `bindings` table.
fn bindings(s: &mut Script) {
    s.schema(
        Table::create()
            .table("bindings")
            .col(text("front").not_null())
            .col(text("conv_key").not_null())
            .col(int("session").not_null())
            .col(text("owner").not_null())
            .col(int("created_at").not_null())
            .primary_key(&mut key(&["front", "conv_key", "session"]))
            .foreign_key(&mut fk("session", "sessions", "id")),
    );
    s.schema(&index("bindings_session", "bindings", &["session"]));
}

/// The `box_profiles` table.
fn box_profiles(s: &mut Script) {
    s.schema(
        Table::create()
            .table("box_profiles")
            .col(int("box").not_null().primary_key())
            .col(text("repo").not_null())
            .col(text("sha").not_null())
            .col(text("manifest"))
            .foreign_key(&mut fk("box", "boxes", "id")),
    );
}

/// The `webhook_deliveries` table.
fn webhook_deliveries(s: &mut Script) {
    let id = row_id(s, "id");
    s.schema(
        Table::create()
            .table("webhook_deliveries")
            .col(id)
            .col(int("webhook").not_null())
            .col(int("event").not_null())
            .col(int("leased_until"))
            .foreign_key(&mut fk("webhook", "user_webhooks", "id"))
            .foreign_key(&mut fk("event", "events", "id")),
    );
    s.schema(&index(
        "webhook_deliveries_webhook",
        "webhook_deliveries",
        &["webhook", "id"],
    ));
}
