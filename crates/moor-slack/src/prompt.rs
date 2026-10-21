//! The message of a needs-input prompt: who is asked, which Box, the escaped question and one
//! button per option, and the same message once answered.

use serde_json::{json, Value};

use crate::escape::escape;

/// Longest button label Slack accepts, in characters.
const LABEL_MAX: usize = 75;
/// Longest text Slack accepts in a section or a context element, in characters.
const SECTION_MAX: usize = 3000;
/// Most elements Slack accepts in an actions block.
const BUTTONS_MAX: usize = 25;
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

/// Cuts escaped `s` to `max` characters as [`clip`] does, without splitting an entity such as
/// `&amp;`: escaped text holds `&` only as an entity's start.
fn clip_escaped(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max - 3).collect();
    match head.rsplit_once('&') {
        Some((whole, part)) if !part.contains(';') => format!("{whole}..."),
        _ => format!("{head}..."),
    }
}

/// Who is asked, which Box, and the escaped question, unclipped.
fn asked(name: &str, requester: &str, question: &str) -> String {
    let head = format!("<@{requester}> *{}* needs input", escape(name));
    if question.is_empty() {
        head
    } else {
        format!("{head}: {}", escape(question))
    }
}

/// Message fallback text: who is asked, which Box, and the escaped question.
pub(crate) fn text(name: &str, requester: &str, question: &str) -> String {
    clip_escaped(&asked(name, requester, question), SECTION_MAX)
}

/// A section block showing escaped `body`, clipped to fit.
fn section(body: &str) -> Value {
    let body = clip_escaped(body, SECTION_MAX);
    json!({"type": "section", "text": {"type": "mrkdwn", "text": body}})
}

/// Blocks for an open prompt: the question, then one button per option up to Slack's limit,
/// then the options past it, which a number in the thread picks.
pub(crate) fn open_blocks(
    name: &str,
    requester: &str,
    question: &str,
    options: &[String],
) -> Value {
    if options.is_empty() {
        let hint = "\nReply in this thread to answer.";
        let room = SECTION_MAX - hint.chars().count();
        let body = format!(
            "{}{hint}",
            clip_escaped(&asked(name, requester, question), room)
        );
        return json!([section(&body)]);
    }
    let buttons: Vec<Value> = options
        .iter()
        .take(BUTTONS_MAX)
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
    let mut blocks = vec![
        section(&asked(name, requester, question)),
        json!({"type": "actions", "block_id": BLOCK_ID, "elements": buttons}),
    ];
    if options.len() > BUTTONS_MAX {
        let hint = "Reply in this thread with the number of an option below to pick it:";
        let more = options
            .iter()
            .enumerate()
            .skip(BUTTONS_MAX)
            .map(|(i, option)| format!("{}. {}", i + 1, escape(option)));
        let lines: Vec<String> = std::iter::once(hint.to_owned()).chain(more).collect();
        blocks.push(section(&lines.join("\n")));
    }
    Value::Array(blocks)
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
    let line = clip_escaped(&line, SECTION_MAX);
    json!([
        section(&asked(name, requester, question)),
        {"type": "context", "elements": [{"type": "mrkdwn", "text": line}]},
    ])
}

/// The answer label for choice `n`: `"<n>. <option>"`, or just `"<n>"` when out of range.
pub(crate) fn answer_label(options: &[String], n: u32) -> String {
    usize::try_from(n)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| options.get(i))
        .map_or_else(|| n.to_string(), |option| format!("{n}. {option}"))
}

/// Whether `n` names one of `options`.
pub(crate) fn offered(options: &[String], n: u32) -> bool {
    usize::try_from(n).is_ok_and(|n| (1..=options.len()).contains(&n))
}

#[cfg(test)]
mod tests;
