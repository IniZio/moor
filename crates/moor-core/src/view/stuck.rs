//! Whether a Box looks stuck, from its events alone.

use serde_json::Value;

use super::progress::Happened;
use super::StuckView;

/// Kinds after which an earlier stuck event no longer counts.
const CLEARS: [&str; 9] = [
    "agent_resumed",
    "turn_done",
    "idled",
    "needs_input",
    "session_started",
    "session_ended",
    "closed",
    "agent_link_restored",
    "proxy_credential_recovered",
];

/// The newest stuck event that nothing since has cleared, `events` oldest first.
///
/// An `egress_recovered` clears only the `egress_aborted` events for its own host, so
/// an older stuck event still uncleared can show again.
#[must_use]
pub fn stuck_of(events: &[Happened]) -> Option<StuckView> {
    // Uncleared stuck events, oldest first. Only aborts can be cleared out of order,
    // so a newer stuck event of any other kind drops everything before it.
    let mut open: Vec<&Happened> = Vec::new();
    for e in events {
        let kind = e.kind.as_str();
        if CLEARS.contains(&kind) {
            open.clear();
        } else if kind == "egress_recovered" {
            let host = text(&e.payload, "host").map(bare_host);
            open.retain(|o| {
                o.kind != "egress_aborted" || text(&o.payload, "host").map(bare_host) != host
            });
        } else if signal(kind).is_some() {
            if kind != "egress_aborted" {
                open.clear();
            }
            open.push(e);
        }
    }
    let e = open.last()?;
    let kind = e.kind.as_str();
    Some(StuckView {
        kind: signal(kind)?.to_owned(),
        since: e.ts.clone(),
        detail: detail(kind, &e.payload),
    })
}

/// `host` lower-cased and without a trailing `:port`.
fn bare_host(host: &str) -> String {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    name.to_ascii_lowercase()
}

/// The stuck name an event kind goes by, when it signals a stuck Box.
fn signal(kind: &str) -> Option<&str> {
    match kind {
        "agent_stalled" => Some("stalled"),
        "egress_refused"
        | "egress_aborted"
        | "credential_withheld"
        | "proxy_credential_failed"
        | "agent_rate_limited"
        | "agent_unreachable"
        | "agent_link_lost" => Some(kind),
        _ => None,
    }
}

/// A non-empty string field.
fn text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// An unsigned number field.
fn number(payload: &Value, key: &str) -> Option<u64> {
    payload.get(key)?.as_u64()
}

/// `head`, with the payload's tool rule when it names one.
fn with_rule(head: String, payload: &Value) -> String {
    match text(payload, "rule") {
        Some(rule) => format!("{head} (rule {rule})"),
        None => head,
    }
}

/// One line saying what happened.
fn detail(kind: &str, p: &Value) -> String {
    let said = match kind {
        "agent_stalled" => number(p, "quiet_secs").map(|secs| {
            let quiet = format!("no hook report for {} min", secs / 60);
            match text(p, "doing") {
                Some(doing) => format!("{quiet} while {doing}"),
                None => quiet,
            }
        }),
        "egress_refused" => text(p, "host").map(|h| with_rule(format!("refused {h}"), p)),
        "egress_aborted" => text(p, "host").map(|h| format!("handshake to {h} aborted")),
        "credential_withheld" => {
            text(p, "host").map(|h| with_rule(format!("credential withheld for {h}"), p))
        }
        "proxy_credential_failed" => {
            text(p, "rule").map(|r| format!("credential for rule {r} failed"))
        }
        "agent_rate_limited" => text(p, "rule").map(|r| {
            number(p, "status").map_or_else(
                || format!("rule {r} rate limited"),
                |status| format!("rule {r} rate limited ({status})"),
            )
        }),
        "agent_unreachable" => match (number(p, "attempts"), text(p, "error")) {
            (Some(n), Some(e)) => Some(format!("unreachable after {n} tries: {e}")),
            _ => text(p, "error").map(str::to_owned),
        },
        "agent_link_lost" => text(p, "pane").map(|pane| {
            p.get("exit").and_then(Value::as_i64).map_or_else(
                || format!("pane {pane} ended"),
                |code| format!("pane {pane} ended with exit {code}"),
            )
        }),
        _ => None,
    };
    let line = said
        .or_else(|| {
            text(p, "error")
                .or_else(|| text(p, "reason"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| kind.replace('_', " "));
    if kind == "egress_aborted" {
        format!("{line}; is the Box clock behind?")
    } else {
        line
    }
}

#[cfg(test)]
mod tests;
