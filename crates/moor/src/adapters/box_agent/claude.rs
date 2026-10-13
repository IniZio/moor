//! The Claude Code profile: hook state file parsing.

use moor_core::activity::{HookKind, HookRecord};
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::clock::Ts;
use serde_json::Value;

/// Parses the state file Claude Code's hooks write.
#[derive(Debug, Default, Clone, Copy)]
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
            "waiting_input" => HookKind::Blocked,
            _ => return None,
        };
        let secs = value.get("ts")?.as_i64().filter(|s| *s >= 0)?;
        let message = if kind == HookKind::Blocked {
            text("message").map(str::to_owned)
        } else {
            None
        };
        Some(HookRecord {
            kind,
            session_id: text("session_id").map(str::to_owned),
            message,
            at: Ts::from_millis(secs.checked_mul(1000)?),
        })
    }

    fn pane_prompt(&self, tail: &str) -> Option<String> {
        let lines: Vec<&str> = tail.lines().collect();
        let starts = |c: char| move |l: &&str| l.trim_start().starts_with(c);
        let top = lines.iter().rposition(starts('╭')).or_else(|| {
            let mut rules = lines.iter().enumerate().filter(|(_, l)| starts('─')(l));
            rules.next_back()?;
            rules.next_back().map(|(i, _)| i)
        })?;
        lines
            .iter()
            .take(top)
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests;
