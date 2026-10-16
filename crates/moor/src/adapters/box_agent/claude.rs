//! The Claude Code profile: launch arguments, hook setup and hook state file parsing.

use moor_core::activity::{HookKind, HookRecord};
use moor_core::name::NameError;
use moor_core::seam::agent_profile::{
    AgentProfile, BoxSetup, EnvVar, FirstRunPrompt, Key, LaunchCtx, PromptMatch, NOTHING_TO_ARCHIVE,
};
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

    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch> {
        let trusting = tail.to_lowercase().contains("trust");
        tail.lines().rev().find_map(|raw| {
            let clean = strip_ansi(raw);
            let line = clean.trim_matches(is_edge);
            prompt_of(&line.to_lowercase(), trusting).map(|prompt| PromptMatch {
                prompt,
                line: line.to_owned(),
            })
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

    fn env(&self, ctx: &LaunchCtx) -> Vec<EnvVar> {
        launch::env(ctx)
    }

    fn busy_marker(&self) -> &'static str {
        "esc to interrupt"
    }

    fn menu_keys(&self, n: u8) -> Vec<Key> {
        launch::menu_keys(n)
    }

    fn transcript_script(&self, archive: &str) -> String {
        format!(
            r#"set -e
enc=$(pwd | sed 's/[^A-Za-z0-9]/-/g')
d="$HOME/.claude/projects/$enc"
mkdir -p "$(dirname {archive})"
if [ ! -d "$d" ]; then
  echo '{NOTHING_TO_ARCHIVE}'
  exit 0
fi
ls "$d" >/dev/null
if ! ls "$d"/*.jsonl >/dev/null 2>&1; then
  echo '{NOTHING_TO_ARCHIVE}'
  exit 0
fi
(cd "$d" && tar czf "$OLDPWD/{archive}" *.jsonl)
test -s "{archive}"
"#
        )
    }
}

/// `line` without ANSI escape sequences: CSI, and OSC ended by BEL or `ESC \`.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for f in chars.by_ref() {
                    if ('@'..='~').contains(&f) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(f) = chars.next() {
                    if f == '\u{7}' {
                        break;
                    }
                    if f == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
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
