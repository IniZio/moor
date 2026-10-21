//! The message of a needs-input prompt: who is asked, which Box, the escaped question and one
//! button per option, and the same message once answered.

use serde_json::{json, Value};

use crate::escape::escape;

/// Longest button label Slack accepts, in characters.
const LABEL_MAX: usize = 75;
/// Longest question shown, in characters; Slack caps a section at 3000.
const QUESTION_MAX: usize = 1000;
/// Stable id of the actions block.
const BLOCK_ID: &str = "moor_prompt";

/// Cuts `s` to `max` characters, ending in `...` when something was dropped.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max - 3).collect();
    format!("{head}...")
}

/// Message fallback text: who is asked, which Box, and the escaped question.
pub(crate) fn text(name: &str, requester: &str, question: &str) -> String {
    let head = format!("<@{requester}> *{}* needs input", escape(name));
    if question.is_empty() {
        head
    } else {
        format!("{head}: {}", escape(&clip(question, QUESTION_MAX)))
    }
}

/// The section block that opens every prompt message.
fn section(name: &str, requester: &str, question: &str, hint: &str) -> Value {
    let body = format!("{}{hint}", text(name, requester, question));
    json!({"type": "section", "text": {"type": "mrkdwn", "text": body}})
}

/// Blocks for an open prompt: the question, then one button per option.
pub(crate) fn open_blocks(
    name: &str,
    requester: &str,
    question: &str,
    options: &[String],
) -> Value {
    if options.is_empty() {
        let hint = "\nReply in this thread to answer.";
        return json!([section(name, requester, question, hint)]);
    }
    let buttons: Vec<Value> = options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let n = i + 1;
            json!({
                "type": "button",
                "action_id": format!("answer_{n}"),
                "value": n.to_string(),
                "text": {"type": "plain_text", "text": clip(option, LABEL_MAX)},
            })
        })
        .collect();
    json!([
        section(name, requester, question, ""),
        {"type": "actions", "block_id": BLOCK_ID, "elements": buttons},
    ])
}

/// Blocks after an answer: the question and who answered, with no buttons.
pub(crate) fn answered_blocks(
    name: &str,
    requester: &str,
    question: &str,
    answer: &str,
    user: &str,
) -> Value {
    let line = format!("Answered by <@{user}>: {}", escape(answer));
    json!([
        section(name, requester, question, ""),
        {"type": "context", "elements": [{"type": "mrkdwn", "text": line}]},
    ])
}

#[cfg(test)]
mod tests;
