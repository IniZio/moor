//! Answering a needs-input prompt: a button press or the owner's bare-number reply.

use moor_core::request::{Request, SteerReq};
use serde_json::Value;

use super::prompt;
use super::socket::Click;
use crate::store::{SlackPrompt, Store};

/// Slack side effects beyond thread replies.
pub(super) trait Ui {
    /// Shows `text` to `user` only, inside `thread`.
    fn ephemeral(&mut self, channel: &str, user: &str, thread: &str, text: &str);
    /// Replaces the text and blocks of message `ts`.
    fn update(&mut self, channel: &str, ts: &str, text: &str, blocks: &Value);
}

/// The effects an answer needs.
pub(super) struct Fx<'a> {
    /// Runs a request on the engine.
    pub(super) call: &'a mut dyn FnMut(Request) -> Result<(), String>,
    /// Posts `(channel, thread, text)` as a thread reply.
    pub(super) reply: &'a mut dyn FnMut(&str, &str, &str),
    /// Ephemeral messages and message edits.
    pub(super) ui: &'a mut dyn Ui,
    /// Controller log.
    pub(super) log: &'a dyn Fn(&str),
}

/// Applies a button press: dedupe, owner check, then the shared answer path.
pub(super) fn handle_click(click: &Click, store: &Store, fx: &mut Fx<'_>) {
    match store.slack_event_once(&click.envelope_id) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    }
    let channel = click.channel.as_str();
    let prompt = match store.slack_prompt_by_message(channel, &click.message_ts) {
        Ok(Some(prompt)) => prompt,
        Ok(None) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    };
    let thread = prompt.thread_ts.as_str();
    let owner = match store.slack_thread_owner(channel, thread) {
        Ok(Some(owner)) => owner,
        Ok(None) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    };
    if click.user != owner {
        let text = format!("Only <@{owner}> can answer this prompt.");
        return fx.ui.ephemeral(channel, &click.user, thread, &text);
    }
    if let Some(answer) = &prompt.answer {
        let text = format!("This prompt was already answered: {answer}.");
        return fx.ui.ephemeral(channel, &click.user, thread, &text);
    }
    answer(&prompt, &owner, click.choice, store, fx);
}

/// Answers the owner's open prompt in `thread` with `choice`, when there is one.
///
/// Returns false when the thread has no open prompt, so the caller treats the text as a steer.
pub(super) fn answer_open(
    channel: &str,
    thread: &str,
    owner: &str,
    choice: u32,
    store: &Store,
    fx: &mut Fx<'_>,
) -> bool {
    match store.open_slack_prompt(channel, thread) {
        Ok(Some(prompt)) => {
            answer(&prompt, owner, choice, store, fx);
            true
        }
        Ok(None) => false,
        Err(e) => {
            (fx.log)(&format!("slack: {e}"));
            true
        }
    }
}

/// Steers menu `choice` when the prompt is still the Box's live question, then records the answer.
fn answer(prompt: &SlackPrompt, owner: &str, choice: u32, store: &Store, fx: &mut Fx<'_>) {
    let channel = prompt.channel.as_str();
    let thread = prompt.thread_ts.as_str();
    let state = match live_state(prompt, store) {
        Ok(None) => None,
        Ok(Some(state)) => Some(state),
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    };
    if let Some(state) = state {
        let text = format!(
            "This prompt is no longer open: {} is now {state}.",
            prompt.box_name.as_str()
        );
        return (fx.reply)(channel, thread, &text);
    }
    let steer = Request::Steer(SteerReq {
        box_name: prompt.box_name.clone(),
        text: None,
        menu: Some(choice),
    });
    if let Err(message) = (fx.call)(steer) {
        return (fx.reply)(channel, thread, &message);
    }
    let label = prompt::answer_label(&prompt.options, choice);
    match store.answer_slack_prompt(prompt.event_id, &label) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => return (fx.log)(&format!("slack: {e}")),
    }
    let name = prompt.box_name.as_str();
    let text = prompt::text(name, owner, &prompt.question);
    let blocks = prompt::answered_blocks(name, owner, &prompt.question, &label, owner);
    fx.ui.update(channel, &prompt.message_ts, &text, &blocks);
}

/// None while `prompt` is the Box's live question; otherwise the state to report.
fn live_state(prompt: &SlackPrompt, store: &Store) -> Result<Option<&'static str>, String> {
    let Some(row) = store
        .open_box_by_name(&prompt.box_name)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Some("closed"));
    };
    let marker = store
        .last_waiting_marker(row.id)
        .map_err(|e| e.to_string())?;
    Ok(match marker {
        Some(m) if m.kind == "needs_input" => {
            (m.id != prompt.event_id).then_some("waiting on a newer question")
        }
        _ => Some("working"),
    })
}
