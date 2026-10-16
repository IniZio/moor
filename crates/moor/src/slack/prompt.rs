//! Prompt messages for Tasks that wait on the owner: one Slack message per waiting episode.

use std::collections::HashMap;

use moor_core::secret::Secret;
use serde_json::{json, Value};

use super::api::Api;
use super::status::escape;
use crate::store::{SlackPrompt, SlackThread, Store};

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

/// Message fallback text: who is asked, which task, and the escaped question.
pub(super) fn text(task: &str, owner: &str, question: &str) -> String {
    let head = format!("<@{owner}> *{}* needs input", escape(task));
    if question.is_empty() {
        head
    } else {
        format!("{head}: {}", escape(&clip(question, QUESTION_MAX)))
    }
}

/// The section block that opens every prompt message.
fn section(task: &str, owner: &str, question: &str, hint: &str) -> Value {
    let body = format!("{}{hint}", text(task, owner, question));
    json!({"type": "section", "text": {"type": "mrkdwn", "text": body}})
}

/// Blocks for an open prompt: the question, then one button per option.
pub(super) fn open_blocks(task: &str, owner: &str, question: &str, options: &[String]) -> Value {
    if options.is_empty() {
        let hint = "\nReply in this thread to answer.";
        return json!([section(task, owner, question, hint)]);
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
        section(task, owner, question, ""),
        {"type": "actions", "block_id": BLOCK_ID, "elements": buttons},
    ])
}

/// Blocks after an answer: the question and who answered, with no buttons.
pub(super) fn answered_blocks(
    task: &str,
    owner: &str,
    question: &str,
    answer: &str,
    user: &str,
) -> Value {
    let line = format!("Answered by <@{user}>: {}", escape(answer));
    json!([
        section(task, owner, question, ""),
        {"type": "context", "elements": [{"type": "mrkdwn", "text": line}]},
    ])
}

/// The answer label for choice `n`: `"<n>. <option>"`, or just `"<n>"` when out of range.
pub(super) fn answer_label(options: &[String], n: u32) -> String {
    usize::try_from(n)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| options.get(i))
        .map_or_else(|| n.to_string(), |option| format!("{n}. {option}"))
}

/// One waiting episode: the `needs_input` event and what it asked.
struct Episode {
    /// Id of the `needs_input` event.
    event_id: i64,
    /// The question, empty when none.
    question: String,
    /// The offered options.
    options: Vec<String>,
}

/// Per-task pending `needs_input` episode, folded from the event feed.
#[derive(Default)]
pub(super) struct Pending {
    /// Open episode by task name.
    open: HashMap<String, Episode>,
}

impl Pending {
    /// Applies one event; a `needs_input` opens (or replaces) the episode, and any event that
    /// moves the Task on closes it.
    pub(super) fn fold(&mut self, event_id: i64, task: &str, kind: &str, payload: &Value) {
        match kind {
            "needs_input" => {
                let question = payload
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let options = payload
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                self.open.insert(
                    task.to_owned(),
                    Episode {
                        event_id,
                        question,
                        options,
                    },
                );
            }
            "working" | "busy" | "woke" | "idled" | "asleep" | "closed" | "rejected"
            | "setup_failed" => {
                self.open.remove(task);
            }
            _ => {}
        }
    }

    /// Posts a prompt for each bound thread whose task has an episode with no recorded prompt.
    /// Failures are logged and retried on the next poll.
    pub(super) fn post_due(
        &self,
        store: &Store,
        api: &Api,
        bot: &Secret,
        threads: &[SlackThread],
        log: &dyn Fn(&str),
    ) {
        for t in threads {
            let Some(ep) = self.open.get(t.task.as_str()) else {
                continue;
            };
            match store.slack_prompt_posted(ep.event_id) {
                Ok(false) => {}
                Ok(true) => continue,
                Err(e) => {
                    log(&format!("slack prompt: {e}"));
                    continue;
                }
            }
            let blocks = open_blocks(t.task.as_str(), &t.owner, &ep.question, &ep.options);
            let fallback = text(t.task.as_str(), &t.owner, &ep.question);
            let ts = match api.post_blocks(bot, &t.channel, &t.thread_ts, &fallback, &blocks) {
                Ok(ts) => ts,
                Err(e) => {
                    log(&format!("slack prompt: {e}"));
                    continue;
                }
            };
            let prompt = SlackPrompt {
                event_id: ep.event_id,
                channel: t.channel.clone(),
                thread_ts: t.thread_ts.clone(),
                message_ts: ts,
                task: t.task.clone(),
                question: ep.question.clone(),
                options: ep.options.clone(),
                answer: None,
            };
            if let Err(e) = store.add_slack_prompt(&prompt) {
                log(&format!("slack prompt: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests;
