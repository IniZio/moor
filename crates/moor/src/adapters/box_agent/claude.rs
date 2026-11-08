//! The Claude Code profile: launch arguments, hook setup and hook state file parsing.

use moor_core::activity::{Note, Question};
use moor_core::name::NameError;
use moor_core::seam::agent_adapter::{
    AgentAdapter, AgentInstall, BoxSetup, EnvVar, FirstRunPrompt, Key, LaunchCtx, PromptMatch,
    NOTHING_TO_ARCHIVE,
};
use serde_json::Value;

mod install;
mod launch;
mod profile;
mod wait;

/// The lines of a JSON array of strings; empty when it is none.
fn lines(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|b| {
            b.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the note Claude Code's hooks leave.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Claude;

impl AgentAdapter for Claude {
    fn parse_note(&self, v: &Value) -> Note {
        let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
        // A question tool's record carries its questions; the first is the one on screen.
        let asked = v
            .get("questions")
            .and_then(Value::as_array)
            .and_then(|q| q.first());
        let question = asked.and_then(|q| {
            Some(Question {
                text: q.get("question")?.as_str()?.to_owned(),
                options: q
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|opts| {
                        opts.iter()
                            .filter_map(|o| o.get("label").and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        });
        Note {
            session_id: text("session_id"),
            said: text("said"),
            question,
            background: lines(v.get("background")),
            background_local: v.get("background_local").and_then(Value::as_bool) == Some(true),
        }
    }

    fn install(&self) -> Option<AgentInstall> {
        Some(install::native())
    }

    fn wait_check_script(&self) -> Option<String> {
        Some(wait::script("/proc"))
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

    fn interrupt_key(&self) -> Key {
        Key::Escape
    }

    fn transcript_script(&self, archive: &str) -> String {
        format!(
            r#"set -e
d="$HOME/.claude/projects/$(pwd -P | sed 's/[^A-Za-z0-9]/-/g')"
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
    if line.contains("new mcp server") {
        return Some(FirstRunPrompt::McpServers);
    }
    if line.contains("allow external claude.md file imports") {
        return Some(FirstRunPrompt::ExternalImports);
    }
    if line.contains("bypass permissions mode") {
        return Some(FirstRunPrompt::BypassPermissions);
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
