//! The Claude Code profile: launch arguments, hook setup and hook state file parsing.

use moor_core::activity::{HookKind, HookRecord};
use moor_core::name::NameError;
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, Key, LaunchCtx};
use moor_core::seam::clock::Ts;
use serde_json::Value;

mod launch;

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

    fn argv(&self, ctx: &LaunchCtx) -> Vec<String> {
        launch::argv(ctx)
    }

    fn resume_argv(&self, session_id: &str, ctx: &LaunchCtx) -> Vec<String> {
        launch::resume_argv(session_id, ctx)
    }

    fn preseed(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
        launch::preseed(ctx)
    }

    fn hook_writer(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
        launch::hook_writer(ctx)
    }

    fn menu_keys(&self, n: u8) -> Vec<Key> {
        launch::menu_keys(n)
    }
}

#[cfg(test)]
mod tests;
