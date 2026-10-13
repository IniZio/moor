//! The Claude Code profile: launch arguments, hook setup and hook state file parsing.

use moor_core::activity::{HookKind, HookRecord};
use moor_core::name::NameError;
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, FirstRunPrompt, Key, LaunchCtx};
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

    fn first_run_prompt(&self, tail: &str) -> Option<FirstRunPrompt> {
        let trusting = tail.to_lowercase().contains("trust");
        tail.lines().rev().find_map(|raw| {
            let line = raw.trim_matches(is_edge).to_lowercase();
            prompt_of(&line, trusting)
        })
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

/// Whitespace and box-drawing characters around a pane line.
fn is_edge(c: char) -> bool {
    c.is_whitespace() || ('\u{2500}'..='\u{257f}').contains(&c)
}

/// The first-run prompt a lowercased pane line shows; `trusting` when the pane mentions trust.
fn prompt_of(line: &str, trusting: bool) -> Option<FirstRunPrompt> {
    const FOLDER: [&str; 3] = [
        "do you trust the files in this folder",
        "is this a project you created or one you trust",
        "trust this folder",
    ];
    if FOLDER.iter().any(|p| line.contains(p)) || (trusting && line.contains("yes, proceed")) {
        return Some(FirstRunPrompt::FolderTrust);
    }
    if line.contains("press enter to continue") {
        return Some(FirstRunPrompt::PressEnter);
    }
    if line.contains("security notes") {
        return Some(FirstRunPrompt::SecurityNotes);
    }
    if line.contains("choose the text style that looks best with your terminal") {
        return Some(FirstRunPrompt::Theme);
    }
    let item = line.trim_start_matches(|c: char| !c.is_alphanumeric());
    let item = item.trim_start_matches(|c: char| c.is_ascii_digit());
    let item = item.strip_prefix('.').unwrap_or(item).trim_start();
    (item.starts_with("dark mode") || item.starts_with("light mode"))
        .then_some(FirstRunPrompt::Theme)
}

#[cfg(test)]
mod tests;
