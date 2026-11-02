//! The stall the agent's hook reported, shown on the status message until it ends.

use serde_json::Value;

use crate::escape::code;

/// A report that the agent went quiet, or that it came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Quiet {
    /// No hook report for `mins` minutes while `doing`.
    Stalled {
        /// Whole minutes without a report.
        mins: u64,
        /// What the agent was doing, when it said.
        doing: Option<String>,
    },
    /// The agent reported again after `mins` minutes.
    Resumed {
        /// Whole minutes the stall lasted.
        mins: u64,
    },
}

/// What the event of `kind` does to the Box's current report: `agent_stalled` and
/// `agent_resumed` set it, and a change of state clears it.
pub(super) fn track(now: &mut Option<Quiet>, kind: &str, payload: &Value, state_changed: bool) {
    let mins = payload
        .get("quiet_secs")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        / 60;
    match kind {
        "agent_stalled" => {
            let doing = payload.get("doing").and_then(Value::as_str);
            *now = Some(Quiet::Stalled {
                mins,
                doing: doing.filter(|d| !d.is_empty()).map(str::to_owned),
            });
        }
        "agent_resumed" => *now = Some(Quiet::Resumed { mins }),
        _ if state_changed => *now = None,
        _ => {}
    }
}

/// Adds the status line for `quiet`, if any, to `text`.
pub(super) fn append(text: &mut String, quiet: Option<&Quiet>) {
    if let Some(quiet) = quiet {
        text.push('\n');
        text.push_str(&line(quiet));
    }
}

/// The status line for `quiet`.
fn line(quiet: &Quiet) -> String {
    match quiet {
        Quiet::Stalled { mins, doing: None } => format!("stalled: no progress for {mins} min"),
        Quiet::Stalled {
            mins,
            doing: Some(d),
        } => format!("stalled: no progress for {mins} min while {}", code(d)),
        Quiet::Resumed { mins } => format!("resumed after {mins} min"),
    }
}
