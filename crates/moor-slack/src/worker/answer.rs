//! Answering a needs-input prompt without the coordinator: a button on the prompt, or a bare
//! number in the thread while exactly one prompt is open. Only the person who asked for the
//! prompt's Box may answer. The Socket Mode thread records the click or message and queues
//! the answer; the worker records it, sends it to the Box, and the feed then shows it.

use moor_core::request::{Request, SteerReq};
use moor_core::response::ExitClass;

use super::{error_text, Fx, Job};
use crate::command::bare_number;
use crate::db::{Db, Prompt, PromptState};
use crate::escape::escape;
use crate::prompt::{answer_label, offered};
use crate::socket::{Click, Mention};

/// Records a button press before Slack gets its ack. Returns what the worker does next, or
/// nothing for a redelivered envelope or a message that is not a prompt.
pub(crate) fn click(click: &Click, db: &Db, now: i64) -> rusqlite::Result<Option<Job>> {
    db.atomically(|| {
        if !db.event_once(&click.envelope_id, now)? {
            return Ok(None);
        }
        let Some(prompt) = db.prompt_by_message(&click.channel, &click.message_ts)? else {
            return Ok(None);
        };
        let tell = |text: String| Job::Ephemeral {
            channel: prompt.channel.clone(),
            user: click.user.clone(),
            thread: prompt.thread_ts.clone(),
            text,
        };
        let by = &prompt.requested_by;
        if &click.user != by {
            return Ok(Some(tell(format!("Only <@{by}> can answer this prompt."))));
        }
        Ok(Some(match &prompt.state {
            PromptState::Open => Job::Answer {
                prompt: prompt.id,
                choice: click.choice,
                by: click.user.clone(),
            },
            PromptState::Answered { label, .. } => tell(already_answered(label)),
            PromptState::Closed(reason) => reply(&prompt, &no_longer_open(&prompt, reason)),
        }))
    })
}

/// The answer a thread message gives when it is a bare number, from the requester of the
/// thread's only open prompt, that names one of the prompt's options. Any other message goes
/// on to the coordinator.
pub(crate) fn digit(db: &Db, mention: &Mention, thread_ts: &str) -> rusqlite::Result<Option<Job>> {
    let Some(choice) = bare_number(&mention.text) else {
        return Ok(None);
    };
    let open = db.open_prompts(&mention.channel, thread_ts)?;
    let [prompt] = open.as_slice() else {
        return Ok(None);
    };
    if prompt.requested_by != mention.user || !offered(&prompt.options, choice) {
        return Ok(None);
    }
    Ok(Some(Job::Answer {
        prompt: prompt.id,
        choice,
        by: mention.user.clone(),
    }))
}

/// Records `choice` as the answer of prompt `id` by `by` while the prompt is still open and
/// offers it, then sends it to the Box. Recording first means the Box moving on while the
/// answer is in flight cannot close the prompt; a Box that does not take it opens the prompt
/// again. A refusal or a failure is posted in the thread.
pub(super) fn answer(db: &Db, fx: &mut Fx<'_>, id: i64, choice: u32, by: &str) {
    let claimed = db.atomically(|| {
        let Some(prompt) = db.prompt(id)? else {
            return Ok(None);
        };
        let take = prompt.state == PromptState::Open && offered(&prompt.options, choice);
        if take {
            db.answer_prompt(id, &answer_label(&prompt.options, choice), by)?;
        }
        Ok(Some((prompt, take)))
    });
    let (prompt, take) = match claimed {
        Ok(Some(claimed)) => claimed,
        Ok(None) => return,
        Err(e) => return (fx.log)(&format!("slack.db: {e}")),
    };
    let said = match &prompt.state {
        PromptState::Open if take => send(db, fx, &prompt, choice),
        PromptState::Open => Some(format!(
            "Pick an option from 1 to {}.",
            prompt.options.len()
        )),
        PromptState::Answered { label, .. } => Some(already_answered(label)),
        PromptState::Closed(reason) => Some(no_longer_open(&prompt, reason)),
    };
    if let Some(text) = said {
        (fx.reply)(&prompt.channel, &prompt.thread_ts, &text);
    }
}

/// Steers `choice` to the Box of `prompt`, whose answer is recorded, and marks it sent; when
/// the Box did not take it, opens the prompt again and returns the line to post.
fn send(db: &Db, fx: &mut Fx<'_>, prompt: &Prompt, choice: u32) -> Option<String> {
    let steer = Request::Steer(SteerReq {
        target: prompt.box_name.clone().into(),
        text: None,
        menu: Some(choice),
        interrupt: false,
    });
    let name = prompt.box_name.as_str();
    let (settled, said) = match (fx.call)(&prompt.requested_by, steer) {
        Ok(reply) if reply.class == ExitClass::Ok => (db.answer_sent(prompt.id), None),
        Ok(reply) => {
            let why = escape(&error_text(reply));
            let said = format!("Box `{name}` refused the answer: {why}");
            (db.reopen_prompt(prompt.id), Some(said))
        }
        Err(e) => {
            let why = escape(&e.to_string());
            let said = format!("The answer may not have reached Box `{name}`: {why}");
            (db.reopen_prompt(prompt.id), Some(said))
        }
    };
    if let Err(e) = settled {
        (fx.log)(&format!("slack.db: {e}"));
    }
    said
}

/// The line for a prompt that already has an answer.
fn already_answered(label: &str) -> String {
    format!("This prompt was already answered: {}.", escape(label))
}

/// The line for a prompt whose Box moved on to `reason`.
fn no_longer_open(prompt: &Prompt, reason: &str) -> String {
    format!(
        "This prompt is no longer open: {} is now {reason}.",
        prompt.box_name.as_str()
    )
}

/// Posts `text` in the thread of `prompt`.
fn reply(prompt: &Prompt, text: &str) -> Job {
    Job::Reply {
        channel: prompt.channel.clone(),
        thread: prompt.thread_ts.clone(),
        text: text.to_owned(),
    }
}
