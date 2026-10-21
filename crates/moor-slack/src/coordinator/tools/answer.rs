//! `answer_box`: picks an option of a question a Box asked, by the id of the prompt the front
//! posted for it. A choice is a bare number, so naming the prompt keeps a repeated answer from
//! picking an option of the Box's next question.

use moor_core::request::{Request, SteerReq};
use moor_core::response::ExitClass;
use serde_json::Value;

use super::{changeable, error_text, not_running, Reply, Scope};
use crate::db::{Prompt, PromptState};
use crate::prompt::{answer_label, offered};

/// The whole number `args` holds under `key`, as a number or as text.
pub(super) fn number<T: TryFrom<i64>>(name: &str, args: &Value, key: &str) -> Result<T, String> {
    let value = args.get(key);
    value
        .and_then(Value::as_i64)
        .or_else(|| value?.as_str()?.trim().parse().ok())
        .and_then(|n| T::try_from(n).ok())
        .ok_or_else(|| format!("{name} needs a whole number `{key}`."))
}

/// `answer_box`: the requester gate, then the prompt must be this Box's, open, and offer
/// `choice`. Then the journal row, `steer` with the choice, and the prompt recorded as
/// answered by the Box's requester.
pub(super) fn answer(
    scope: &mut Scope<'_>,
    box_name: &str,
    id: i64,
    choice: u32,
) -> rusqlite::Result<Reply> {
    let args = format!("prompt {id}");
    let target = match changeable(scope, "answer_box", box_name, &args)? {
        Ok(target) => target,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(refusal) = not_running(&target.boxed, "answer") {
        return Ok(refusal);
    }
    let name = target.boxed.box_name.as_str().to_owned();
    let prompt = scope.db.prompt(id)?.filter(|p| p.box_name.as_str() == name);
    let prompt = match refused(prompt, &name, id, choice) {
        Ok(prompt) => prompt,
        Err(refusal) => return Ok(refusal),
    };
    let db = scope.db;
    let seq = db.begin_action(target.turn.id, &target.action, (scope.clock)())?;
    let request = Request::Steer(SteerReq {
        target: target.boxed.box_name.clone().into(),
        text: None,
        menu: Some(choice),
    });
    let label = answer_label(&prompt.options, choice);
    let (outcome, reply) = match (scope.call)(request) {
        Ok(r) if r.class == ExitClass::Ok => {
            db.answer_prompt(id, &label, &target.action.requested_by)?;
            let said = format!("answered prompt {id} of Box `{name}` with {label}");
            let reply = Reply::done(format!("Answered prompt {id} with {label}."));
            (said, reply)
        }
        Ok(r) => {
            let why = error_text(r);
            let reply = Reply::refused(format!("Not answered: {why}"));
            (format!("the controller refused: {why}"), reply)
        }
        Err(e) => {
            let said = format!("the controller did not answer: {e}");
            let reply = Reply::refused(format!("Maybe not answered: {said}"));
            (said, reply)
        }
    };
    db.end_action(target.turn.id, seq, &outcome, (scope.clock)())?;
    Ok(reply)
}

/// The prompt to answer, or why `prompt` cannot take `choice`: it is not one of Box `name`'s,
/// it is no longer open, or it offers no such option.
fn refused(prompt: Option<Prompt>, name: &str, id: i64, choice: u32) -> Result<Prompt, Reply> {
    let Some(prompt) = prompt else {
        return Err(Reply::refused(format!(
            "Box `{name}` has no prompt {id}. Call list_boxes for its open prompts."
        )));
    };
    match &prompt.state {
        PromptState::Open => {}
        PromptState::Answered { label, .. } => {
            return Err(Reply::refused(format!(
                "Prompt {id} is already answered: {label}."
            )));
        }
        PromptState::Closed(reason) => {
            return Err(Reply::refused(format!(
                "Prompt {id} is no longer open: Box `{name}` is now {reason}."
            )));
        }
    }
    if offered(&prompt.options, choice) {
        return Ok(prompt);
    }
    let n = prompt.options.len();
    Err(Reply::refused(if n == 0 {
        format!("Prompt {id} offers no options. Steer the Box with text instead.")
    } else {
        format!("Prompt {id} has options 1 to {n}, not {choice}.")
    }))
}
