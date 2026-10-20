//! The `/moor` slash command. `/moor repo` lists and changes the repos a channel works on,
//! recorded before the ack; the worker then warns in the channel about a repo the
//! controller does not know.

use moor_core::config::{find_repo, RepoEntry, RepoTable};
use moor_core::request::{ReposReq, Request};
use moor_core::response::ExitClass;
use moor_core::view::repos::ReposValue;
use serde_json::{json, Value};

use crate::command::Allow;
use crate::db::{ChannelRepo, Db};
use crate::socket::Slash;
use crate::worker::{Fx, Job};

/// Reply for text that is not a well-formed `/moor` command.
const USAGE: &str = "Usage: `/moor repo` lists this channel's repos. \
    `/moor repo add <owner/name>`, `/moor repo remove <owner/name>` and \
    `/moor repo default <owner/name>` change them.";

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

/// Records one command in a single transaction with its envelope id. Returns the ack's
/// payload and, for a repo newly added by a first delivery, the worker's repo check.
pub(crate) fn record(
    slash: &Slash,
    db: &Db,
    allow: &Allow<'_>,
    now: i64,
) -> rusqlite::Result<(Value, Option<Job>)> {
    db.atomically(|| {
        let first = db.event_once(&slash.envelope_id, now)?;
        let (text, added) = answer(slash, db, allow, now)?;
        let job = added.filter(|_| first).map(|repo| Job::CheckRepo {
            channel: slash.channel.clone(),
            repo,
        });
        Ok((json!({ "text": text }), job))
    })
}

/// Applies the command; returns the text the user sees and the repo it newly added.
fn answer(
    slash: &Slash,
    db: &Db,
    allow: &Allow<'_>,
    now: i64,
) -> rusqlite::Result<(String, Option<String>)> {
    let Some(cmd) = parse(&slash.text) else {
        return Ok((USAGE.to_owned(), None));
    };
    if !allow.channels.contains(&slash.channel) {
        return Ok(("moor does not work in this channel.".to_owned(), None));
    }
    if !allow.users.contains(&slash.user) {
        let user = &slash.user;
        let text = format!("<@{user}> is not allowed to change this channel's repos.");
        return Ok((text, None));
    }
    let channel = slash.channel.as_str();
    let not_here = |repo: &str| format!("`{repo}` is not one of this channel's repos.");
    Ok(match cmd {
        RepoCmd::List => (listing(&db.channel_repos(channel)?), None),
        RepoCmd::Add(repo) if db.add_channel_repo(channel, repo, &slash.user, now)? => (
            format!("Recorded. This channel works on `{repo}`."),
            Some(repo.to_owned()),
        ),
        RepoCmd::Add(repo) => (format!("This channel already works on `{repo}`."), None),
        RepoCmd::Remove(repo) if db.remove_channel_repo(channel, repo)? => {
            (format!("Removed `{repo}` from this channel."), None)
        }
        RepoCmd::Default(repo) if db.set_default_channel_repo(channel, repo)? => (
            format!("Recorded. `{repo}` is this channel's default repo."),
            None,
        ),
        RepoCmd::Remove(repo) | RepoCmd::Default(repo) => (not_here(repo), None),
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

/// Asks the controller for its repos and posts one line in `channel` when `repo` is not
/// one of them, by name or clone URL. A failed request is logged; the repo stays recorded.
pub(crate) fn check_repo(fx: &mut Fx<'_>, channel: &str, repo: &str) {
    let known = match (fx.call)(Request::Repos(ReposReq {})) {
        Ok(reply) if reply.class == ExitClass::Ok => reply
            .value
            .and_then(|v| serde_json::from_value::<ReposValue>(v).ok())
            .ok_or_else(|| "the repos reply is not a repo list".to_owned()),
        Ok(reply) => Err(reply
            .error
            .map_or_else(|| "repos failed".to_owned(), |e| e.message)),
        Err(e) => Err(e.to_string()),
    };
    let known = match known {
        Ok(known) => known,
        Err(why) => return (fx.log)(&format!("repo check of {repo}: {why}")),
    };
    let table: RepoTable = known
        .repos
        .iter()
        .map(|r| {
            let entry = RepoEntry {
                url: r.url.clone(),
                default_branch: None,
                moor_yaml: None,
            };
            (r.name.clone(), entry)
        })
        .collect();
    if find_repo(&table, repo).is_some() {
        return;
    }
    let names: Vec<String> = table.keys().map(|n| format!("`{n}`")).collect();
    let known = if names.is_empty() {
        "It knows no repos.".to_owned()
    } else {
        format!("Known repos: {}.", names.join(", "))
    };
    let text = format!("The controller does not know `{repo}`. {known}");
    (fx.say)(channel, &text);
}

#[cfg(test)]
mod tests;
