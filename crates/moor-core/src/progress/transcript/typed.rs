//! The messages a person typed straight into the agent's terminal, told from everything else
//! Claude records as a user turn.

use serde_json::Value;

use super::text_of;
use crate::workflow::{is_draft_prompt, split_plan_first, split_start_prompt};

/// How a note Claude or another agent made for the model opens: a tag with a hyphen in its name,
/// as slash commands, shell escapes, system notes and other agents' messages do, or an
/// interrupt.
fn wrapped(text: &str) -> bool {
    let tag = text
        .strip_prefix('<')
        .and_then(|rest| rest.split(|c: char| c == '>' || c.is_whitespace()).next());
    tag.is_some_and(|tag| tag.contains('-')) || text.starts_with("[Request interrupted by user")
}

/// The words of user record `v` when a person typed them into the terminal. None for a tool
/// result, a note Claude or a hook added, a message from another agent or session, and a prompt
/// moor made itself.
pub(super) fn prompt(v: &Value) -> Option<String> {
    let flag = |key: &str| v.get(key).and_then(Value::as_bool) == Some(true);
    if flag("isMeta") || flag("isCompactSummary") || flag("isVisibleInTranscriptOnly") {
        return None;
    }
    if v.pointer("/origin/kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "human")
    {
        return None;
    }
    let text = match v.pointer("/message/content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|b| text_of(b, "type") == "tool_result") {
                return None;
            }
            blocks
                .iter()
                .filter(|b| text_of(b, "type") == "text")
                .map(|b| text_of(b, "text"))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !wrapped(text) && !made_by_moor(text)).then(|| text.to_owned())
}

/// Whether `text` is one of the prompts moor builds for an agent: a plan draft, a plan-first
/// prompt or a workflow step's start prompt.
fn made_by_moor(text: &str) -> bool {
    is_draft_prompt(text) || split_plan_first(text).is_some() || split_start_prompt(text).is_some()
}

#[cfg(test)]
mod tests;
