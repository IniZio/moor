//! The `/moor` slash command. `/moor repo` lists and changes the repos a channel works on,
//! recorded before the ack. `/moor login` and `/moor logout` go to [`crate::login`].

use moor_core::seam::forge::ForgeRepo;
use serde_json::{json, Value};

use crate::db::{ChannelRepo, Db};
use crate::login;
use crate::socket::{Kept, Slash};
use crate::worker::Job;

/// Reply for text that is not a well-formed `/moor` command.
const USAGE: &str = "Usage: `/moor repo` lists this channel's repos. \
    `/moor repo add <owner/name>`, `/moor repo remove <owner/name>` and \
    `/moor repo default <owner/name>` change them. \
    `/moor login [claude|github]` links a token of yours and \
    `/moor logout [claude|github]` unlinks it.";

/// Longest repo name taken.
const MAX_REPO: usize = 200;

/// One `/moor repo` command.
#[derive(Debug, PartialEq, Eq)]
enum RepoCmd<'a> {
    /// List the channel's repos.
    List,
    /// Add a repo to the channel.
    Add(&'a str),
    /// Remove a repo from the channel.
    Remove(&'a str),
    /// Mark one of the channel's repos as its default.
    Default(&'a str),
}

/// Handles one `/moor` command before its ack. Returns what the ack carries and the
/// worker's job, if any.
pub(crate) fn record(
    slash: &Slash,
    db: &Db,
    channels: &[String],
    now: i64,
) -> rusqlite::Result<(Kept, Option<Job>)> {
    let verb = slash.text.split_whitespace().next().unwrap_or_default();
    if verb.eq_ignore_ascii_case("login") {
        return login::login(slash, db, now).map(|kept| (kept, None));
    }
    if verb.eq_ignore_ascii_case("logout") {
        return login::logout(slash, db, now);
    }
    let ack = record_repo(slash, db, channels, now)?;
    let kept = Kept {
        ack: Some(ack),
        then: None,
    };
    Ok((kept, None))
}

/// Records one `/moor repo` command in a single transaction with its envelope id. Returns
/// the ack's payload.
fn record_repo(slash: &Slash, db: &Db, channels: &[String], now: i64) -> rusqlite::Result<Value> {
    db.atomically(|| {
        db.event_once(&slash.envelope_id, now)?;
        let text = answer(slash, db, channels, now)?;
        Ok(json!({ "text": text }))
    })
}

/// Applies the command; returns the text the user sees.
fn answer(slash: &Slash, db: &Db, channels: &[String], now: i64) -> rusqlite::Result<String> {
    let Some(cmd) = parse(&slash.text) else {
        return Ok(USAGE.to_owned());
    };
    if !channels.contains(&slash.channel) {
        return Ok("moor does not work in this channel.".to_owned());
    }
    let channel = slash.channel.as_str();
    let not_here = |repo: &str| format!("`{repo}` is not one of this channel's repos.");
    Ok(match cmd {
        RepoCmd::List => listing(&db.channel_repos(channel)?),
        RepoCmd::Add(repo) if ForgeRepo::parse(repo).is_err() => {
            format!("`{repo}` is not a GitHub repo name. Use `/moor repo add <owner/name>`.")
        }
        RepoCmd::Add(repo) if db.add_channel_repo(channel, repo, &slash.user, now)? => {
            format!("Recorded. This channel works on `{repo}`.")
        }
        RepoCmd::Add(repo) => format!("This channel already works on `{repo}`."),
        RepoCmd::Remove(repo) if db.remove_channel_repo(channel, repo)? => {
            format!("Removed `{repo}` from this channel.")
        }
        RepoCmd::Default(repo) if db.set_default_channel_repo(channel, repo)? => {
            format!("Recorded. `{repo}` is this channel's default repo.")
        }
        RepoCmd::Remove(repo) | RepoCmd::Default(repo) => not_here(repo),
    })
}

/// Parses `repo`, `repo add|remove|default <name>`; `None` for anything else.
fn parse(text: &str) -> Option<RepoCmd<'_>> {
    let mut words = text.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("repo") {
        return None;
    }
    let cmd = match (words.next(), words.next()) {
        (None, _) => RepoCmd::List,
        (Some(verb), Some(repo)) if repo_name_ok(repo) => {
            match verb.to_ascii_lowercase().as_str() {
                "add" => RepoCmd::Add(repo),
                "remove" => RepoCmd::Remove(repo),
                "default" => RepoCmd::Default(repo),
                _ => return None,
            }
        }
        _ => return None,
    };
    words.next().is_none().then_some(cmd)
}

/// True for a name made of letters, digits, `.`, `_`, `/` and `-`, so a repo name posted
/// back to Slack can carry no mention or link.
fn repo_name_ok(repo: &str) -> bool {
    repo.len() <= MAX_REPO
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
}

/// The channel's repos as one line, the default marked.
fn listing(repos: &[ChannelRepo]) -> String {
    if repos.is_empty() {
        return "This channel has no repos. Add one with `/moor repo add <owner/name>`.".to_owned();
    }
    let names: Vec<String> = repos
        .iter()
        .map(|r| {
            let mark = if r.is_default { " (default)" } else { "" };
            format!("`{}`{mark}", r.repo)
        })
        .collect();
    format!("This channel works on {}.", names.join(", "))
}

#[cfg(test)]
mod tests;
