//! How far a starting Box has got, told from its events in the words a person reads.

use serde::Deserialize;
use serde_json::Value;

/// How many of a Box's latest events the card reads.
pub const EVENTS: u32 = 40;

/// How many steps the card shows; older ones scroll off.
const SHOWN: usize = 8;

/// One of a Box's events, as `show` sends it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Happened {
    /// The snake-case kind.
    pub kind: String,
    /// The kind's fields.
    #[serde(default)]
    pub payload: Value,
    /// When, as RFC 3339 UTC.
    pub ts: String,
}

/// A starting Box's steps so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Seconds since the Box was started, when its events tell.
    pub elapsed: Option<u64>,
    /// What happened, oldest first, each with its time since the start.
    pub steps: Vec<String>,
}

impl Progress {
    /// The progress `events`, oldest first, tell at `now` (unix seconds).
    #[must_use]
    pub fn of(events: &[Happened], now: i64) -> Self {
        let began = events
            .iter()
            .find(|e| e.kind == "started")
            .or_else(|| events.first())
            .and_then(|e| unix(&e.ts));
        let steps: Vec<String> = events
            .iter()
            .filter_map(|e| {
                let text = step(&e.kind, &e.payload)?;
                Some(match unix(&e.ts).zip(began) {
                    Some((at, began)) => format!("{:>7}  {text}", format!("+{}", span(at - began))),
                    None => text,
                })
            })
            .collect();
        let skip = steps.len().saturating_sub(SHOWN);
        Self {
            elapsed: began.map(|b| u64::try_from(now - b).unwrap_or(0)),
            steps: steps.into_iter().skip(skip).collect(),
        }
    }
}

/// Why a Box is down: its last failure, unless a close came after it.
#[must_use]
pub fn failure(events: &[Happened]) -> Option<String> {
    events
        .iter()
        .rev()
        .take_while(|e| e.kind != "closed")
        .find(|e| {
            matches!(
                e.kind.as_str(),
                "setup_failed" | "rejected" | "box_failed" | "credential_failed" | "session_failed"
            )
        })
        .and_then(|e| step(&e.kind, &e.payload))
}

/// The newest step `events` tell, without its time.
#[must_use]
pub fn latest(events: &[Happened]) -> Option<String> {
    events.iter().rev().find_map(|e| step(&e.kind, &e.payload))
}

/// Event `kind` as a step, or `None` for one that says nothing about getting the Box up.
fn step(kind: &str, payload: &Value) -> Option<String> {
    let text = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or_default();
    let took = || {
        payload
            .get("ms")
            .and_then(Value::as_u64)
            .map_or_else(String::new, |ms| format!(" in {}", span(ms / 1000)))
    };
    Some(match kind {
        "pending" => "waiting for a machine slot".to_owned(),
        "box_requested" => "machine requested".to_owned(),
        "box_progress" => text("text").to_owned(),
        "box_ready" => "machine ready".to_owned(),
        "box_failed" => format!("machine failed: {}", text("error")),
        "setting_up" => "setting up".to_owned(),
        "cloned" => format!("cloned{}", took()),
        "setup_finished" if payload.get("ok").and_then(Value::as_bool) == Some(false) => {
            format!("setup finished with failures{}", took())
        }
        "setup_finished" => format!("setup finished{}", took()),
        "setup_failed" => format!("setup failed: {}", text("line")),
        "credential_failed" => format!("credential failed: {}: {}", text("which"), text("error")),
        "rejected" => format!("rejected: {}", text("reason")),
        "session_started" => "agent started".to_owned(),
        "session_failed" => format!(
            "agent failed to start: {}: {}",
            text("step"),
            text("message")
        ),
        "launch_timed_out" => "the agent is slow to start".to_owned(),
        "agent_prompt_blocked" => format!("the agent waits at a prompt: {}", text("prompt")),
        _ => return None,
    })
}

/// `secs` as `12s` or `1m 05s`.
pub fn span(secs: impl TryInto<u64>) -> String {
    let s = secs.try_into().unwrap_or(0);
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// An RFC 3339 UTC time in whole seconds, as the controller writes it, in unix seconds.
fn unix(ts: &str) -> Option<i64> {
    let num = |at: usize, len: usize| ts.get(at..at + len)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let secs = num(11, 2)? * 3600 + num(14, 2)? * 60 + num(17, 2)?;
    // Days since 1970-01-01 in the proleptic Gregorian calendar, years starting in March.
    let y = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(days * 86_400 + secs)
}

#[cfg(test)]
mod tests;
