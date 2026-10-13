//! The Claude Code profile: hook state file parsing.

use moor_core::activity::{HookKind, HookRecord};
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::clock::Ts;
use serde_json::Value;

/// Parses the state file Claude Code's hooks write.
#[derive(Debug, Default, Clone, Copy)]
#[cfg_attr(not(test), expect(dead_code, reason = "wired in wire.rs"))]
pub(crate) struct Claude;

impl AgentProfile for Claude {
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord> {
        let value: Value = serde_json::from_slice(raw).ok()?;
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        let idle_prompt = text("notification_type") == Some("idle_prompt");
        let kind = match text("state")? {
            _ if idle_prompt => HookKind::Idle,
            "starting" => HookKind::Starting,
            "working" => HookKind::Busy,
            "idle" => HookKind::Idle,
            "waiting_input" => HookKind::Blocked {
                waiting_for: text("message").unwrap_or_default().to_owned(),
            },
            _ => return None,
        };
        let secs = value.get("ts")?.as_i64().filter(|s| *s >= 0)?;
        Some(HookRecord {
            kind,
            session_id: text("session_id").map(str::to_owned),
            at: Ts::from_millis(secs.checked_mul(1000)?),
        })
    }
}

#[cfg(test)]
mod tests;
