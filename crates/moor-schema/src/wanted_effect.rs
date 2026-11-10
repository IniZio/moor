//! Keep what moor wants a Box to end as, and the external step in flight for it, in columns of
//! the Box's own row, so no decision about the Box reads its event history as state.
//!
//! `box_state` gains the wanted action, the effect in flight, a note owed to the agent and when
//! the relaunch cap was recorded. `launches` gains why each launch began, on which hands, and
//! the agent facts that were events; `proxy` gains its last tunnel restart. Each is filled from
//! the events that held it until now. The intent columns stay, unread, for a later step to drop.

use moor_migrate::sea_query::Table;
use moor_migrate::{Backend, Script};

use crate::{int, text};

/// The `box_state` columns this step adds, in order.
const STATE: [(&str, bool); 10] = [
    ("wanted", true),
    ("wanted_force", false),
    ("wanted_reason", true),
    ("wanted_at", false),
    ("effect", true),
    ("effect_attempt", false),
    ("effect_run", false),
    ("effect_at", false),
    ("owed_note", true),
    ("relaunch_capped_at", false),
];

/// The `launches` columns this step adds, in order.
const LAUNCH: [(&str, bool); 5] = [
    ("cause", true),
    ("hands_inc", false),
    ("stalled_at", false),
    ("unreachable_since", false),
    ("exit_save_at", false),
];

/// The reason an `asleep` event gives for a machine that stopped under its Box, which the note
/// owed after such a stop names. It matches `moor_core::decide::reconcile::STOPPED_REASON` as it
/// read when this step was written; an applied step never changes.
const STOPPED_REASON: &str = "its machine stopped, as when its host restarts";

/// Add the columns, every one nullable, and fill them from each Box's intent and events.
pub(super) fn write(s: &mut Script) {
    for (table, cols) in [("box_state", &STATE[..]), ("launches", &LAUNCH[..])] {
        for (name, is_text) in cols {
            let col = if *is_text { text(name) } else { int(name) };
            s.schema(&Table::alter().table(table).add_column(col).take());
        }
    }
    s.schema(
        &Table::alter()
            .table("proxy")
            .add_column(int("restarted_at"))
            .take(),
    );
    let field = |key: &str| match s.backend() {
        Backend::Sqlite => format!("json_extract(\"e\".\"payload\", '$.{key}')"),
        Backend::Postgres => format!("(CAST(\"e\".\"payload\" AS jsonb) ->> '{key}')"),
    };
    let (cause, reason, trigger) = (field("cause"), field("reason"), field("trigger"));
    for sql in effect() {
        s.raw(&sql);
    }
    for sql in launches(&cause, &trigger) {
        s.raw(&sql);
    }
    for sql in notes(&reason) {
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

/// The effect and wanted action each intent stood for, and how many times the acquire of a start
/// in flight has asked for a machine since the start began.
fn effect() -> Vec<String> {
    let of = "\"box_state\".\"box\"";
    let setting_up = newest("id", "setting_up", of);
    let asked = format!(
        "(SELECT COUNT(*) FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {of} \
         AND \"e\".\"kind\" = 'box_requested' AND \"e\".\"id\" > COALESCE({setting_up}, 0))"
    );
    vec![
        "UPDATE \"box_state\" SET \
         \"effect\" = CASE \"intent_kind\" WHEN 'provisioning' THEN 'acquire' \
         WHEN 'setup' THEN 'setup' ELSE 'run' END, \
         \"effect_run\" = CASE WHEN \"intent_kind\" = 'running' THEN \"intent_run\" END, \
         \"effect_at\" = \"intent_at\" \
         WHERE \"intent_kind\" IN ('provisioning', 'setup', 'running')"
            .to_owned(),
        format!(
            "UPDATE \"box_state\" SET \"effect_attempt\" = \
             CASE WHEN {asked} > 1 THEN {asked} ELSE 1 END WHERE \"effect\" IS NOT NULL"
        ),
        "UPDATE \"box_state\" SET \"wanted\" = 'close', \"wanted_force\" = 1, \
         \"wanted_at\" = \"intent_at\" WHERE \"intent_kind\" = 'closing'"
            .to_owned(),
    ]
}

/// Each launch's cause and hands, and the agent facts of each Box's live launch.
fn launches(cause: &str, trigger: &str) -> Vec<String> {
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
    let (stalled, stalled_id, resumed_id, restored) = (
        since("ts", "agent_stalled"),
        since("id", "agent_stalled"),
        since("id", "agent_resumed"),
        since("ts", "agent_link_restored"),
    );
    let unreachable = format!(
        "(SELECT MIN(\"e\".\"ts\") FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {l}.\"box\" \
         AND \"e\".\"kind\" = 'agent_unreachable' AND \"e\".\"ts\" >= {l}.\"started_at\" \
         AND \"e\".\"ts\" >= COALESCE({restored}, 0))"
    );
    let saved = format!(
        "(SELECT MAX(\"e\".\"ts\") FROM \"events\" AS \"e\" WHERE \"e\".\"box\" = {l}.\"box\" \
         AND \"e\".\"kind\" IN ('transcript_saved', 'transcript_failed', 'transcript_skipped') \
         AND {trigger} = 'exit' AND \"e\".\"ts\" >= {l}.\"started_at\")"
    );
    vec![
        format!("UPDATE {l} SET \"cause\" = {caused}, \"hands_inc\" = {hands}"),
        format!(
            "UPDATE {l} SET \
             \"stalled_at\" = CASE WHEN {resumed_id} IS NULL OR {resumed_id} < {stalled_id} \
             THEN {stalled} END, \
             \"unreachable_since\" = {unreachable}, \"exit_save_at\" = {saved} \
             WHERE \"ended_at\" IS NULL"
        ),
    ]
}

/// The note each Box owes its agent, when the relaunch cap was recorded, and each tunnel's last
/// restart.
fn notes(reason: &str) -> Vec<String> {
    let of = "\"box_state\".\"box\"";
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
             WHERE \"resume_first_at\" IS NOT NULL"
        ),
        format!(
            "UPDATE \"box_state\" SET \"owed_note\" = '{{\"kind\":\"disk_lost\"}}' \
             WHERE \"owed_note\" IS NULL AND {lost} IS NOT NULL \
             AND ({started} IS NULL OR {started} < {lost}) AND NOT {live}"
        ),
        format!(
            "UPDATE \"box_state\" SET \"relaunch_capped_at\" = {capped} \
             WHERE {capped_id} IS NOT NULL AND ({relaunched} IS NULL OR {relaunched} < {capped_id})"
        ),
        format!(
            "UPDATE \"proxy\" SET \"restarted_at\" = {}",
            newest("ts", "tunnel_restarted", "\"proxy\".\"box\"")
        ),
    ]
}
