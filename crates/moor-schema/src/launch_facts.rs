//! Fill what each Box's events said about its agent into the rows the first binary to read them
//! uses: the cause and hands of recent launches, the facts of each live launch, the note owed to
//! the agent, when the relaunch cap was recorded, each tunnel's last restart and when live hands
//! last woke, a column this step adds.
//!
//! Every fill is bounded, so the step stays short on a store with millions of events: causes only
//! for launches inside the window the relaunch cap counts, launch facts only for live launches,
//! and Box facts only for open Boxes.

use moor_migrate::sea_query::Table;
use moor_migrate::{Backend, Script};

use crate::int;

/// The window the relaunch cap counts over, in milliseconds; an older launch's cause is never
/// read.
const CAP_WINDOW_MS: i64 = 10 * 60 * 1000;

/// The reason an `asleep` event gives for a machine that stopped under its Box, as moor wrote it
/// when this step was written; an applied step never changes.
const STOPPED_REASON: &str = "its machine stopped, as when its host restarts";

/// Add `box_hands.woke_at`, then fill the launch, Box, Proxy and hands facts from the events.
pub(super) fn write(s: &mut Script) {
    s.schema(
        &Table::alter()
            .table("box_hands")
            .add_column(int("woke_at"))
            .take(),
    );
    let backend = s.backend();
    let field = |key: &str| match backend {
        Backend::Sqlite => format!("json_extract(\"e\".\"payload\", '$.{key}')"),
        Backend::Postgres => format!("(CAST(\"e\".\"payload\" AS jsonb) ->> '{key}')"),
    };
    let now = match backend {
        Backend::Sqlite => "(CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
        Backend::Postgres => "(CAST(EXTRACT(EPOCH FROM now()) AS BIGINT) * 1000)",
    };
    for sql in launches(&field("cause"), &field("trigger"), now) {
        s.raw(&sql);
    }
    for sql in boxes(&field("reason"), now) {
        s.raw(&sql);
    }
}

/// The greatest `col`, `ts` or `id`, of Box `of`'s events of `kind`, as a scalar subquery. Order
/// is by `id`: two events can share a `ts`.
fn newest(col: &str, kind: &str, of: &str) -> String {
    format!(
        "(SELECT MAX(\"e\".\"{col}\") FROM \"events\" AS \"e\" \
         WHERE \"e\".\"box\" = {of} AND \"e\".\"kind\" = '{kind}')"
    )
}

/// The cause of each launch inside the cap window, and the hands and agent facts of each live
/// launch.
fn launches(cause: &str, trigger: &str, now: &str) -> Vec<String> {
    let l = "\"launches\"";
    // A relaunch's event follows the launch it began, before any later launch.
    let caused = format!(
        "(SELECT {cause} FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {l}.\"box\" \
         AND \"e\".\"kind\" = 'relaunched' AND {cause} IS NOT NULL \
         AND \"e\".\"ts\" >= {l}.\"started_at\" \
         AND NOT EXISTS (SELECT 1 FROM \"launches\" AS \"n\" WHERE \"n\".\"box\" = {l}.\"box\" \
         AND \"n\".\"id\" > {l}.\"id\" AND \"n\".\"started_at\" <= \"e\".\"ts\") \
         ORDER BY \"e\".\"id\" DESC LIMIT 1)"
    );
    let hands = format!(
        "(SELECT MAX(\"h\".\"inc\") FROM \"box_hands\" AS \"h\" \
         WHERE \"h\".\"box\" = {l}.\"box\" AND \"h\".\"created_at\" <= {l}.\"started_at\")"
    );
    let since = |col: &str, kind: &str| {
        format!(
            "(SELECT MAX(\"e\".\"{col}\") FROM \"events\" AS \"e\" \
             WHERE \"e\".\"box\" = {l}.\"box\" AND \"e\".\"kind\" = '{kind}' \
             AND \"e\".\"ts\" >= {l}.\"started_at\")"
        )
    };
    let (stalled, stalled_id, resumed_id, restored_id) = (
        since("ts", "agent_stalled"),
        since("id", "agent_stalled"),
        since("id", "agent_resumed"),
        since("id", "agent_link_restored"),
    );
    let unreachable = format!(
        "(SELECT MIN(\"e\".\"ts\") FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {l}.\"box\" \
         AND \"e\".\"kind\" = 'agent_unreachable' AND \"e\".\"ts\" >= {l}.\"started_at\" \
         AND \"e\".\"id\" > COALESCE({restored_id}, 0))"
    );
    let saved = format!(
        "(SELECT MAX(\"e\".\"ts\") FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {l}.\"box\" \
         AND \"e\".\"kind\" IN ('transcript_saved', 'transcript_failed', 'transcript_skipped') \
         AND {trigger} = 'exit' AND \"e\".\"ts\" >= {l}.\"started_at\")"
    );
    vec![
        format!(
            "UPDATE {l} SET \"cause\" = {caused} \
             WHERE \"cause\" IS NULL AND \"started_at\" > {now} - {CAP_WINDOW_MS}"
        ),
        format!(
            "UPDATE {l} SET \"hands_inc\" = {hands}, \
             \"stalled_at\" = CASE WHEN {resumed_id} IS NULL OR {resumed_id} < {stalled_id} \
             THEN {stalled} END, \
             \"unreachable_since\" = {unreachable}, \"exit_save_at\" = {saved} \
             WHERE \"ended_at\" IS NULL"
        ),
    ]
}

/// The note each open Box owes its agent, when its relaunch cap was recorded, each tunnel's
/// restart and when each Box's live hands woke, the last two inside the cap window, the longest
/// any reader looks back.
fn boxes(reason: &str, now: &str) -> Vec<String> {
    let since = format!("({now} - {CAP_WINDOW_MS})");
    let of = "\"box_state\".\"box\"";
    let open = "\"state\" NOT IN ('closed', 'rejected')";
    let asleep = format!(
        "(SELECT {reason} FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {of} \
         AND \"e\".\"kind\" = 'asleep' ORDER BY \"e\".\"id\" DESC LIMIT 1)"
    );
    let live = format!(
        "EXISTS (SELECT 1 FROM \"launches\" AS \"l\" WHERE \"l\".\"box\" = {of} \
         AND \"l\".\"ended_at\" IS NULL)"
    );
    let (lost, started) = (
        newest("id", "disk_lost", of),
        newest("id", "session_started", of),
    );
    let (capped, capped_id, relaunched) = (
        newest("ts", "relaunch_capped", of),
        newest("id", "relaunch_capped", of),
        newest("id", "relaunched", of),
    );
    vec![
        format!(
            "UPDATE \"box_state\" SET \"owed_note\" = \
             '{{\"kind\":\"suspended\",\"at\":' || CAST(\"resume_first_at\" AS TEXT) \
             || ',\"stopped\":' || CASE WHEN {asleep} = '{STOPPED_REASON}' \
             THEN 'true' ELSE 'false' END || '}}' \
             WHERE {open} AND \"owed_note\" IS NULL AND \"resume_first_at\" IS NOT NULL"
        ),
        format!(
            "UPDATE \"box_state\" SET \"owed_note\" = '{{\"kind\":\"disk_lost\"}}' \
             WHERE {open} AND \"owed_note\" IS NULL AND {lost} IS NOT NULL \
             AND ({started} IS NULL OR {started} < {lost}) AND NOT {live}"
        ),
        format!(
            "UPDATE \"box_state\" SET \"relaunch_capped_at\" = {capped} \
             WHERE {open} AND \"relaunch_capped_at\" IS NULL AND {capped_id} IS NOT NULL \
             AND ({relaunched} IS NULL OR {relaunched} < {capped_id})"
        ),
        format!(
            "UPDATE \"proxy\" SET \"restarted_at\" = (SELECT MAX(\"e\".\"ts\") \
             FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = \"proxy\".\"box\" \
             AND \"e\".\"kind\" = 'tunnel_restarted' AND \"e\".\"ts\" >= {since}) \
             WHERE \"restarted_at\" IS NULL"
        ),
        format!(
            "UPDATE \"box_hands\" SET \"woke_at\" = (SELECT MAX(\"e\".\"ts\") \
             FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = \"box_hands\".\"box\" \
             AND \"e\".\"kind\" = 'woke' AND \"e\".\"ts\" >= {since} \
             AND \"e\".\"ts\" >= \"box_hands\".\"created_at\") \
             WHERE \"ended_at\" IS NULL AND \"state\" = 'ready'"
        ),
    ]
}
