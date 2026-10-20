//! `/moor login` and `/moor logout`. A person pastes their own agent credential into a
//! modal; the worker checks it with the agent before it stores the credential file. With
//! no agent named, the commands mean [`AgentKind::DEFAULT`].

use std::fmt;
use std::path::Path;

use moor_core::secret::Secret;
use serde_json::{json, Value};

use crate::credentials::{self, AgentKind, CheckError, CredentialCheck, SlackUser};
use crate::db::Db;
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

/// `callback_id` of the login modal.
pub(crate) const LOGIN_CALLBACK: &str = "moor_login";

/// Block id and action id of the modal's credential input.
pub(crate) const TOKEN_INPUT: &str = "token";

/// The modal while the worker checks the credential.
const CHECKING: &str = "Checking your token…";

/// The modal after the credential file is written.
const LINKED: &str = "Linked. Your token pays for the coordinator in threads you start.";

/// The ack of a user whose Slack id cannot be a file name.
const BAD_ID: &str = "Your Slack user id cannot name a token file, so nothing was changed.";

/// A credential to check for `user`, then store, and the modal to report the outcome in.
pub(crate) struct TokenCheck {
    /// The user who pasted it.
    pub(crate) user: SlackUser,
    /// The agent it is for.
    pub(crate) agent: AgentKind,
    /// The modal's view id.
    pub(crate) view_id: String,
    /// The credential.
    pub(crate) token: Secret,
}

impl fmt::Debug for TokenCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCheck")
            .field("user", &self.user)
            .field("agent", &self.agent)
            .field("view_id", &self.view_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TokenCheck {
    fn eq(&self, other: &Self) -> bool {
        self.user == other.user
            && self.agent == other.agent
            && self.view_id == other.view_id
            && self.token.digest() == other.token.digest()
    }
}

impl Eq for TokenCheck {}

/// The modal `/moor login` opens for `agent`; its `private_metadata` names the agent.
pub(crate) fn form(agent: AgentKind) -> Value {
    json!({
        "type": "modal",
        "callback_id": LOGIN_CALLBACK,
        "private_metadata": agent.name(),
        "title": {"type": "plain_text", "text": "moor login"},
        "submit": {"type": "plain_text", "text": "Link"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "blocks": [{
            "type": "input",
            "block_id": TOKEN_INPUT,
            "label": {"type": "plain_text", "text": agent.login_prompt()},
            "element": {"type": "plain_text_input", "action_id": TOKEN_INPUT},
        }],
    })
}

/// A modal that shows `text` as plain text, so nothing in it is a mention or a link.
pub(crate) fn notice(text: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": LOGIN_CALLBACK,
        "title": {"type": "plain_text", "text": "moor login"},
        "close": {"type": "plain_text", "text": "Close"},
        "blocks": [{"type": "section", "text": {"type": "plain_text", "text": text}}],
    })
}

/// `/moor login [agent]`: an allowed user gets an empty ack, then the modal. The envelope
/// id is recorded so a redelivered command opens no second modal.
pub(crate) fn login(slash: &Slash, db: &Db, users: &[String], now: i64) -> rusqlite::Result<Kept> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    if !users.contains(&slash.user) {
        let user = &slash.user;
        return Ok(text_ack(&format!(
            "<@{user}> is not allowed to link a token here."
        )));
    }
    let agent = match agent_named(&slash.text) {
        Ok(agent) => agent,
        Err(kept) => return Ok(kept),
    };
    if SlackUser::parse(&slash.user).is_none() {
        return Ok(text_ack(BAD_ID));
    }
    let then = slash
        .trigger_id
        .clone()
        .filter(|_| first)
        .map(|trigger_id| AfterAck::OpenLogin { trigger_id, agent });
    Ok(Kept { ack: None, then })
}

/// `/moor logout [agent]`: deletes the user's credential file for the agent. A running turn
/// keeps the credential it read.
pub(crate) fn logout(slash: &Slash, state_dir: &Path, log: &dyn Fn(&str)) -> Kept {
    let agent = match agent_named(&slash.text) {
        Ok(agent) => agent,
        Err(kept) => return kept,
    };
    let Some(user) = SlackUser::parse(&slash.user) else {
        return text_ack(BAD_ID);
    };
    match credentials::remove(state_dir, agent, &user) {
        Ok(()) => text_ack("Unlinked."),
        Err(e) => {
            log(&format!("logout of {}: {e}", user.as_str()));
            text_ack("Could not delete your token file. The operator's log says why.")
        }
    }
}

/// The agent the word after `login` or `logout` names, or the default when there is none;
/// an ack that names the known agents otherwise.
fn agent_named(text: &str) -> Result<AgentKind, Kept> {
    let mut words = text.split_whitespace().skip(1);
    match (words.next(), words.next()) {
        (None, _) => Ok(AgentKind::DEFAULT),
        (Some(name), None) => AgentKind::parse(name).ok_or_else(|| {
            let known = AgentKind::DEFAULT.name();
            text_ack(&format!(
                "moor knows no agent called that. It knows `{known}`."
            ))
        }),
        _ => Err(text_ack(
            "Usage: `/moor login [agent]` or `/moor logout [agent]`.",
        )),
    }
}

/// Keeps a submitted modal: records only its envelope id, and on the first delivery from an
/// allowed user queues the check. The ack turns the modal into "Checking your token…".
pub(crate) fn submission(
    sub: TokenSubmission,
    db: &Db,
    users: &[String],
    now: i64,
) -> rusqlite::Result<(Value, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&sub.envelope_id, now))?;
    let user = SlackUser::parse(&sub.user).filter(|_| users.contains(&sub.user));
    let Some(user) = user else {
        let text = "You are not allowed to link a token here. Nothing was stored.";
        return Ok((update_ack(text), None));
    };
    let Some(agent) = AgentKind::parse(&sub.agent) else {
        let text = "This form names no agent moor knows. Nothing was stored.";
        return Ok((update_ack(text), None));
    };
    let job = first.then(|| {
        Job::CheckToken(TokenCheck {
            user,
            agent,
            view_id: sub.view_id,
            token: sub.token,
        })
    });
    Ok((update_ack(CHECKING), job))
}

/// Checks the credential with its agent and stores it only when the agent takes it; returns
/// the text the modal shows. `checker` is the check of the job's agent, or why it cannot
/// run. Never carries the credential.
pub(crate) fn check(
    job: &TokenCheck,
    checker: Result<&dyn CredentialCheck, &str>,
    state_dir: &Path,
) -> String {
    let checker = match checker {
        Ok(checker) => checker,
        Err(why) => return unavailable(why),
    };
    let label = job.agent.label();
    if let Err(why) = checker.check_format(&job.token) {
        return refused(label, &why);
    }
    let config_dir = match credentials::config_dir(state_dir, job.agent, &job.user) {
        Ok(dir) => dir,
        Err(why) => return unavailable(&why),
    };
    match checker.check(&config_dir, &job.token) {
        Ok(()) => match credentials::store(state_dir, job.agent, &job.user, &job.token) {
            Ok(()) => LINKED.to_owned(),
            Err(why) => format!("Could not store your token: {why}. Nothing was stored."),
        },
        Err(CheckError::Refused(why)) => refused(label, &why),
        Err(CheckError::Unavailable(why)) => unavailable(&why),
    }
}

/// The modal text for a credential the agent refused.
fn refused(agent: &str, why: &str) -> String {
    let why = why.trim_end_matches('.');
    format!("{agent} refused this token: {why}. Nothing was stored.")
}

/// The modal text for a check that could not ask the agent.
fn unavailable(why: &str) -> String {
    let why = why.trim_end_matches('.');
    format!("The coordinator is unavailable: {why}. Nothing was stored.")
}

/// An ack that shows `text` to the user who ran the command.
fn text_ack(text: &str) -> Kept {
    Kept {
        ack: Some(json!({ "text": text })),
        then: None,
    }
}

/// A submission ack that replaces the modal with `text`.
fn update_ack(text: &str) -> Value {
    json!({"response_action": "update", "view": notice(text)})
}

#[cfg(test)]
mod tests;
