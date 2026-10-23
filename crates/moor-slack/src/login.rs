//! `/moor login` and `/moor logout`. A person pastes their own token into a modal; the worker
//! checks it with the account, then stores it in the controller's vault as that Slack user.
//! With no word, the commands mean [`Account::DEFAULT`].

use std::fmt;
use std::path::Path;

use moor_core::secret::Secret;
use serde_json::{json, Value};

use crate::account::Account;
use crate::credentials::{self, AgentKind, CheckError, CredentialCheck, SlackUser};
use crate::db::Db;
use crate::github::GitHub;
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

pub(crate) mod vault;

/// `callback_id` of the login modal.
pub(crate) const LOGIN_CALLBACK: &str = "moor_login";

/// Block id and action id of the modal's credential input.
pub(crate) const TOKEN_INPUT: &str = "token";

/// The modal while the worker checks the credential.
const CHECKING: &str = "Checking your token…";

/// The modal after the secret is in the vault.
pub(crate) const LINKED: &str = "Linked.";

/// The ack of a user whose Slack id the front cannot act for.
const BAD_ID: &str = "Your Slack user id is not one moor can act for, so nothing was changed.";

/// A credential to check for `user`, then store, and the modal to report the outcome in.
pub(crate) struct TokenCheck {
    /// The user who pasted it.
    pub(crate) user: SlackUser,
    /// The account it is for.
    pub(crate) account: Account,
    /// The modal's view id.
    pub(crate) view_id: String,
    /// The credential.
    pub(crate) token: Secret,
}

impl fmt::Debug for TokenCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCheck")
            .field("user", &self.user)
            .field("account", &self.account)
            .field("view_id", &self.view_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TokenCheck {
    fn eq(&self, other: &Self) -> bool {
        self.user == other.user
            && self.account == other.account
            && self.view_id == other.view_id
            && self.token.digest() == other.token.digest()
    }
}

impl Eq for TokenCheck {}

/// The modal `/moor login` opens for `account`; its `private_metadata` names the account.
pub(crate) fn form(account: Account) -> Value {
    json!({
        "type": "modal",
        "callback_id": LOGIN_CALLBACK,
        "private_metadata": account.name(),
        "title": {"type": "plain_text", "text": "moor login"},
        "submit": {"type": "plain_text", "text": "Link"},
        "close": {"type": "plain_text", "text": "Cancel"},
        "blocks": [{
            "type": "input",
            "block_id": TOKEN_INPUT,
            "label": {"type": "plain_text", "text": account.login_prompt()},
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

/// `/moor login [account]`: anyone gets an empty ack, then the modal. The envelope id is
/// recorded so a redelivered command opens no second modal.
pub(crate) fn login(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<Kept> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    let account = match account_named(&slash.text) {
        Ok(account) => account,
        Err(kept) => return Ok(kept),
    };
    if SlackUser::parse(&slash.user).is_none() {
        return Ok(text_ack(BAD_ID));
    }
    let then = slash
        .trigger_id
        .clone()
        .filter(|_| first)
        .map(|trigger_id| AfterAck::OpenLogin {
            trigger_id,
            account,
        });
    Ok(Kept { ack: None, then })
}

/// `/moor logout [account]`: acks at once and queues the delete of the user's secret, which
/// the worker runs as that user and reports through the command's `response_url`.
pub(crate) fn logout(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<(Kept, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    let account = match account_named(&slash.text) {
        Ok(account) => account,
        Err(kept) => return Ok((kept, None)),
    };
    if SlackUser::parse(&slash.user).is_none() {
        return Ok((text_ack(BAD_ID), None));
    }
    let job = first.then(|| Job::Unlink {
        user: slash.user.clone(),
        account,
        response_url: slash.response_url.clone(),
    });
    let label = account.label();
    Ok((text_ack(&format!("Unlinking your {label} token…")), job))
}

/// The account the word after `login` or `logout` names, or the default when there is none;
/// an ack that names the known accounts otherwise.
fn account_named(text: &str) -> Result<Account, Kept> {
    let mut words = text.split_whitespace().skip(1);
    match (words.next(), words.next()) {
        (None, _) => Ok(Account::DEFAULT),
        (Some(name), None) => Account::parse(name)
            .ok_or_else(|| text_ack("moor cannot link that. It can link `claude` and `github`.")),
        _ => Err(text_ack(
            "Usage: `/moor login [claude|github]` or `/moor logout [claude|github]`.",
        )),
    }
}

/// Keeps a submitted modal: records only its envelope id, and on the first delivery queues the
/// check. The ack turns the modal into "Checking your token…".
pub(crate) fn submission(
    sub: TokenSubmission,
    db: &Db,
    now: i64,
) -> rusqlite::Result<(Value, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&sub.envelope_id, now))?;
    let Some(user) = SlackUser::parse(&sub.user) else {
        return Ok((update_ack(&format!("{BAD_ID} Nothing was stored.")), None));
    };
    let Some(account) = Account::parse(&sub.account) else {
        let text = "This form names no account moor knows. Nothing was stored.";
        return Ok((update_ack(text), None));
    };
    let job = first.then(|| {
        Job::CheckToken(TokenCheck {
            user,
            account,
            view_id: sub.view_id,
            token: sub.token,
        })
    });
    Ok((update_ack(CHECKING), job))
}

/// The checks of every account, so the worker can run the one a token is for.
pub(crate) struct Checks<'a> {
    /// The coordinator's check of a Claude token, or why it cannot run.
    pub(crate) claude: Result<&'a dyn CredentialCheck, &'a str>,
    /// GitHub, which takes a token it answers `GET /user` for.
    pub(crate) github: &'a GitHub,
    /// The front's state dir, which holds each user's config dir for the Claude check.
    pub(crate) state_dir: &'a Path,
}

impl Checks<'_> {
    /// Checks the credential with its account; `Err` is the text the modal shows, which says
    /// that nothing was stored. Never carries the credential.
    pub(crate) fn run(&self, job: &TokenCheck) -> Result<(), String> {
        let outcome = match job.account {
            Account::Claude => self.claude(job),
            Account::GitHub => GitHub::check_format(&job.token)
                .map_err(CheckError::Refused)
                .and_then(|()| self.github.check(&job.token)),
        };
        outcome.map_err(|e| match e {
            CheckError::Refused(why) => refused(job.account.label(), &why),
            CheckError::Unavailable(why) => unavailable(job.account, &why),
        })
    }

    /// The coordinator's check: the format, then one call to Claude in the user's own config dir.
    fn claude(&self, job: &TokenCheck) -> Result<(), CheckError> {
        let checker = self
            .claude
            .map_err(|why| CheckError::Unavailable(why.to_owned()))?;
        checker
            .check_format(&job.token)
            .map_err(CheckError::Refused)?;
        let dir = credentials::config_dir(self.state_dir, AgentKind::Claude, &job.user)
            .map_err(CheckError::Unavailable)?;
        checker.check(&dir, &job.token)
    }
}

/// The modal text for a credential the account refused.
fn refused(account: &str, why: &str) -> String {
    let why = why.trim_end_matches('.');
    format!("{account} refused this token: {why}. Nothing was stored.")
}

/// The modal text for a check that could not ask the account.
fn unavailable(account: Account, why: &str) -> String {
    let why = why.trim_end_matches('.');
    match account {
        Account::Claude => format!("The coordinator is unavailable: {why}. Nothing was stored."),
        Account::GitHub => format!("GitHub is unavailable: {why}. Nothing was stored."),
    }
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
