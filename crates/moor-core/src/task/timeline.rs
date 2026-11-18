//! The task's timeline: what a person reads of a Box's events, each classed so a front folds
//! the routine ones and never an error.

use serde::Serialize;
use serde_json::Value;

use crate::view::progress::step;
use crate::view::EventView;

/// How a front treats one timeline entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// The agent said something.
    Message,
    /// A tool call: one line, foldable with its neighbours.
    Step,
    /// A message from a person to the agent.
    You,
    /// The agent stopped to ask.
    Asked,
    /// A turn ended.
    Turn,
    /// An artifact was published.
    Evidence,
    /// Something went wrong. Never folded.
    Error,
    /// Setup and lifecycle notes: foldable.
    Routine,
    /// Not shown.
    Hidden,
}

impl Class {
    /// How the timeline treats events of `kind`; none for a kind no one has classed, which a
    /// front must show, never fold. A test keeps this total over every event kind.
    #[must_use]
    #[allow(clippy::too_many_lines, reason = "one arm per event kind")]
    pub fn of(kind: &str) -> Option<Self> {
        Some(match kind {
            "agent_said" => Self::Message,
            "agent_doing" => Self::Step,
            "steered" => Self::You,
            "needs_input" => Self::Asked,
            "turn_done" => Self::Turn,
            "artifact_published" => Self::Evidence,
            "rejected"
            | "setup_failed"
            | "credential_failed"
            | "box_failed"
            | "box_release_failed"
            | "close_refused"
            | "session_failed"
            | "launch_timed_out"
            | "relaunch_capped"
            | "death_capped"
            | "agent_unreachable"
            | "agent_stalled"
            | "steer_cut"
            | "steers_dropped"
            | "run_failed"
            | "agent_rate_limited"
            | "proxy_credential_rejected"
            | "proxy_credential_failed"
            | "egress_refused"
            | "transcript_failed"
            | "disk_lost"
            | "session_lost"
            | "agent_prompt_check_failed"
            | "steer_unconfirmed"
            | "node_disk_low"
            | "profile_failed"
            | "agent_link_lost"
            | "credential_withheld"
            | "machine_forward_failed"
            | "machine_refused"
            | "agent_prompt_blocked" => Self::Error,
            "started"
            | "pending"
            | "setting_up"
            | "working"
            | "idled"
            | "busy"
            | "asleep"
            | "woke"
            | "closed"
            | "box_requested"
            | "box_progress"
            | "box_ready"
            | "box_asleep"
            | "box_gone"
            | "box_deleted"
            | "session_started"
            | "session_ended"
            | "setup_finished"
            | "cloned"
            | "step_started"
            | "step_finished"
            | "setup_line_started"
            | "setup_line_finished"
            | "setup_reattached"
            | "acquire_reattached"
            | "relaunched"
            | "agent_resumed"
            | "agent_link_restored"
            | "proxy_credential_recovered"
            | "session_restored"
            | "run_waiting"
            | "cache_cold"
            | "ran" => Self::Routine,
            "caches_not_saved"
            | "wanted_dropped"
            | "stays_up"
            | "copied"
            | "exec_ran"
            | "port_forward_opened"
            | "port_forward_closed"
            | "port_forward_refused"
            | "sleep_held"
            | "checked"
            | "run_started"
            | "attached"
            | "sessions_stopped"
            | "prompt_not_given"
            | "previous_box_kept"
            | "transcript_saved"
            | "transcript_skipped"
            | "proxy_started"
            | "tunnel_restarted"
            | "egress_aborted"
            | "egress_recovered"
            | "egress_seen"
            | "node_disk_recovered"
            | "machine_attached"
            | "machine_left_out"
            | "machine_detached"
            | "machine_waiting"
            | "profile_resolved"
            | "cache_emptied" => Self::Hidden,
            _ => return None,
        })
    }
}

/// One line of the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineEntry {
    /// The event's id.
    pub id: i64,
    /// RFC 3339 UTC time.
    pub at: String,
    /// The event kind.
    pub kind: String,
    /// How to treat it.
    pub class: Class,
    /// What to show.
    pub text: String,
    /// For evidence, the artifact's name and version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<(String, i64)>,
}

/// The first of `keys` that `payload` has as text.
fn field<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| payload.get(k).and_then(Value::as_str))
        .filter(|s| !s.trim().is_empty())
}

/// The words for an event.
fn words(e: &EventView) -> String {
    let p = &e.payload;
    let own = match e.kind.as_str() {
        "agent_said" | "agent_doing" => field(p, &["text"]).map(str::to_owned),
        "steered" => field(p, &["message"]).map(str::to_owned),
        "needs_input" => Some(
            field(p, &["question"]).map_or_else(|| "Needs your input".to_owned(), str::to_owned),
        ),
        "turn_done" => Some(format!(
            "Turn ended ({})",
            field(p, &["outcome"]).unwrap_or("idle").replace('_', " ")
        )),
        "artifact_published" => field(p, &["name"]).map(str::to_owned),
        _ => step(&e.kind, p),
    };
    own.unwrap_or_else(|| {
        let kind = e.kind.replace('_', " ");
        match field(p, &["message", "error", "reason", "line", "text", "why"]) {
            Some(why) => format!("{kind}: {why}"),
            None => kind,
        }
    })
}

/// The timeline of `events`, oldest first, without the hidden kinds. A kind no one classed shows
/// as an error so it is never folded away.
#[must_use]
pub fn timeline(events: &[EventView]) -> Vec<TimelineEntry> {
    events
        .iter()
        .filter_map(|e| {
            let class = Class::of(&e.kind).unwrap_or(Class::Error);
            if class == Class::Hidden {
                return None;
            }
            let artifact = (class == Class::Evidence)
                .then(|| {
                    Some((
                        e.payload.get("name")?.as_str()?.to_owned(),
                        e.payload.get("version")?.as_i64()?,
                    ))
                })
                .flatten();
            Some(TimelineEntry {
                id: e.id,
                at: e.ts.clone(),
                kind: e.kind.clone(),
                class,
                text: words(e),
                artifact,
            })
        })
        .collect()
}
