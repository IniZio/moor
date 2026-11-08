//! `/moor login` and `/moor logout`. Tokens are linked on the account page, never in Slack:
//! login answers with its link, and logout unlinks one account's token for the Slack user.

use serde_json::json;

use crate::account::Account;
use crate::credentials::SlackUser;
use crate::db::Db;
use crate::socket::{Kept, Slash};
use crate::worker::Job;

pub(crate) mod vault;

/// The ack of a user whose Slack id the front cannot act for.
const BAD_ID: &str = "Your Slack user id is not one moor can act for, so nothing was changed.";

/// `/moor login`: points at the account page, which is the one place to link a token. The
/// address is the operator's `account_url`; without it the reply says so.
pub(crate) fn login(account_url: Option<&str>) -> Kept {
    let text = account_url.map_or_else(
        || {
            "Link your Claude and GitHub tokens on your moor's account page, the address you \
             sign in at. Ask the operator to set `account_url` so this reply can link it."
                .to_owned()
        },
        |url| {
            format!(
                "Link your Claude and GitHub tokens on your account page:\n{}/\n\
                 Paste each token there; moor never takes one in Slack.",
                url.trim_end_matches('/')
            )
        },
    );
    text_ack(&text)
}

/// `/moor logout [account]`: acks at once and queues the delete of the user's secret, which
/// the worker runs as that user and reports through the command's `response_url`.
pub(crate) fn logout(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<(Kept, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    let account = match account_named(&slash.text) {
        Ok(account) => account.unwrap_or(Account::DEFAULT),
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

/// The account the word after `logout` names, or none when there is no word;
/// an ack that names the known accounts otherwise.
fn account_named(text: &str) -> Result<Option<Account>, Kept> {
    let mut words = text.split_whitespace().skip(1);
    match (words.next(), words.next()) {
        (None, _) => Ok(None),
        (Some(name), None) => Account::parse(name).map(Some).ok_or_else(|| {
            text_ack("moor cannot unlink that. It can unlink `claude` and `github`.")
        }),
        _ => Err(text_ack("Usage: `/moor logout [claude|github]`.")),
    }
}

/// An ack that shows `text` to the user who ran the command.
fn text_ack(text: &str) -> Kept {
    Kept {
        ack: Some(json!({ "text": text })),
        then: None,
    }
}

#[cfg(test)]
mod tests;
