//! `/moor profile [set <url>[#ref]|remove]`: the git repo a person keeps their agent profile in,
//! sent to the controller as that Slack user. Every answer goes to them alone. These are setup
//! commands; nothing reads words in a thread.

use moor_core::name::ProfileSource;
use moor_core::request::{AsReq, EnrollReq, ProfileSetReq, Request};
use moor_core::view::user::ProfileValue;
use serde_json::Value;

use crate::credentials::SlackUser;
use crate::db::Db;
use crate::escape::{code, escape};
use crate::login::vault::{send, Call};
use crate::mcp::{text_ack, BAD_ID};
use crate::socket::{Kept, Slash};
use crate::worker::Job;

/// The reply to a `/moor profile` that is not one of the three.
const USAGE: &str = "Usage: `/moor profile` shows your agent profile, \
    `/moor profile set https://github.com/OWNER/NAME[#REF]` sets it and \
    `/moor profile remove` clears it.";

/// What a `/moor profile` asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Show the profile.
    Show,
    /// Replace the profile.
    Set(ProfileSource),
    /// Clear the profile.
    Remove,
}

/// A `/moor profile` for the worker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProfileJob {
    /// The Slack user who ran the command.
    pub(crate) user: String,
    /// What they asked for.
    pub(crate) change: Change,
    /// Where Slack takes the answer to the command.
    pub(crate) response_url: Option<String>,
}

/// Parses the words after `profile`; the error is the text to show.
fn parse(words: &[&str]) -> Result<Change, String> {
    match words {
        [] => Ok(Change::Show),
        ["remove"] => Ok(Change::Remove),
        ["set", url] => ProfileSource::parse(url)
            .map(Change::Set)
            .map_err(|e| format!("{e}. Nothing was changed.")),
        _ => Err(USAGE.to_owned()),
    }
}

/// `/moor profile ...`: queues a job whose answer goes to the user alone through the command's
/// `response_url`.
pub(crate) fn slash(slash: &Slash, db: &Db, now: i64) -> rusqlite::Result<(Kept, Option<Job>)> {
    let first = db.atomically(|| db.event_once(&slash.envelope_id, now))?;
    if SlackUser::parse(&slash.user).is_none() {
        return Ok((text_ack(BAD_ID), None));
    }
    let words: Vec<&str> = slash.text.split_whitespace().skip(1).collect();
    let (kept, job) = match parse(&words) {
        Ok(change) => {
            let ack = match change {
                Change::Show => "Looking up your profile…",
                Change::Set(_) => "Setting your profile…",
                Change::Remove => "Clearing your profile…",
            };
            let job = Job::Profile(ProfileJob {
                user: slash.user.clone(),
                change,
                response_url: slash.response_url.clone(),
            });
            (text_ack(ack), Some(job))
        }
        Err(text) => (text_ack(&text), None),
    };
    Ok((kept, job.filter(|_| first)))
}

/// The profile the controller holds for `user`, or its error text.
fn current(call: &mut Call<'_>, user: &str) -> Result<Option<ProfileSource>, String> {
    let request = Request::ProfileShow(AsReq { identity: None });
    let reply = send(call, user, request)?;
    let value = reply.value.unwrap_or(Value::Null);
    serde_json::from_value::<ProfileValue>(value)
        .map(|v| v.source)
        .map_err(|e| e.to_string())
}

/// The answer to `job`.
pub(crate) fn answer(call: &mut Call<'_>, job: &ProfileJob) -> String {
    let user = job.user.as_str();
    match &job.change {
        Change::Show => match current(call, user) {
            Ok(Some(source)) => format!("Your agent profile is {}.", code(&source.to_string())),
            Ok(None) => "You have no agent profile. Set one with \
                `/moor profile set https://github.com/OWNER/NAME[#REF]`."
                .to_owned(),
            Err(why) => format!("Could not look up your profile: {}", escape(&why)),
        },
        Change::Set(source) => set(call, user, source),
        Change::Remove => match current(call, user) {
            Ok(None) => "You had no agent profile.".to_owned(),
            Ok(Some(_)) => {
                let request = Request::ProfileRemove(AsReq { identity: None });
                match send(call, user, request) {
                    Ok(_) => "Removed your agent profile.".to_owned(),
                    Err(why) => format!("Could not remove your profile: {}", escape(&why)),
                }
            }
            Err(why) => format!("Could not remove your profile: {}", escape(&why)),
        },
    }
}

/// Enrolls `user` when new, then records `source`.
fn set(call: &mut Call<'_>, user: &str, source: &ProfileSource) -> String {
    let enroll = Request::Enroll(EnrollReq {
        identity: None,
        email: None,
    });
    if let Err(why) = send(call, user, enroll) {
        return format!(
            "Could not set you up: {}. Nothing was stored.",
            escape(&why)
        );
    }
    let request = Request::ProfileSet(ProfileSetReq {
        identity: None,
        source: source.clone(),
    });
    match send(call, user, request) {
        Ok(_) => format!(
            "Recorded. Each new Box applies {}.",
            code(&source.to_string())
        ),
        Err(why) => format!("Could not set your profile: {}", escape(&why)),
    }
}

#[cfg(test)]
mod tests;
