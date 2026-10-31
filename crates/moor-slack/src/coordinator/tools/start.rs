//! `start_box`: the one tool that adds a Box to a thread.

use moor_core::name::{BoxName, SessionId};

use super::{Reply, Scope};
use crate::coordinator::context::{action_line, Action};
use crate::db::{BoxState, ChannelRepo, OpenTurn, SlackBox};

/// `start_box`: the journal row and a `starting` Box row, in one transaction. The Box asks for
/// the thread's host Session as its parent. The worker sends the start and posts its result in
/// the thread.
pub(super) fn start(
    scope: &Scope<'_>,
    prompt: &str,
    (repo, base, provider): (Option<&str>, Option<&str>, Option<&str>),
) -> rusqlite::Result<Reply> {
    let (db, channel, thread_ts) = (scope.db, scope.channel, scope.thread_ts);
    let Some(turn) = db.open_turn(channel, thread_ts)? else {
        return Ok(Reply::refused("No turn of this thread is running."));
    };
    let Some(thread) = db.thread(channel, thread_ts)? else {
        return Ok(Reply::refused("The front keeps no record of this thread."));
    };
    let by = thread.owner;
    if let Some(refusal) = start_refused(scope, &turn, &by)? {
        return Ok(refusal);
    }
    let repo = match pick_repo(&db.channel_repos(channel)?, repo) {
        Ok(repo) => repo,
        Err(refusal) => return Ok(Reply::refused(refusal)),
    };
    let journal = db.thread_actions(channel, thread_ts)?;
    let n = journal.iter().filter(|a| a.tool == "start_box").count() + 1;
    let name = format!("slack-{}-{n}", thread_ts.replace('.', "-"));
    let Ok(box_name) = BoxName::parse_allow_bare(&name) else {
        return Ok(Reply::refused(format!("`{name}` is not a valid Box name.")));
    };
    let row = SlackBox {
        channel: channel.to_owned(),
        thread_ts: thread_ts.to_owned(),
        box_name,
        repo: repo.clone(),
        prompt: prompt.to_owned(),
        base: base.map(str::to_owned),
        provider: provider.map(str::to_owned),
        requested_by: by.clone(),
        parent: db.root_session(channel, thread_ts)?.map(SessionId::new),
        state: BoxState::Starting,
    };
    let action = Action {
        turn: turn.id,
        requested_by: by,
        tool: "start_box".to_owned(),
        box_name: name.clone(),
        args: format!("{prompt} (repo {repo})"),
        outcome: None,
    };
    let outcome = format!("Box `{name}` is starting on {repo}");
    let now = (scope.clock)();
    db.atomically(|| {
        let seq = db.begin_action(turn.id, &action, now)?;
        db.insert_starting(&row, now)?;
        db.end_action(turn.id, seq, &outcome, now)
    })?;
    Ok(Reply::done(format!(
        "{outcome}. It has not started yet and can still fail, so do not tell the thread it \
         started; the thread will say whether it did."
    )))
}

/// Why `start_box` may not start a Box now: the starter `by` wrote nothing in this turn, a
/// rerun's cut-off turn already started one, or the thread is at its open-Box limit.
fn start_refused(scope: &Scope<'_>, turn: &OpenTurn, by: &str) -> rusqlite::Result<Option<Reply>> {
    let db = scope.db;
    if !turn.authors.iter().any(|a| a == by) {
        return Ok(Some(Reply::refused(format!(
            "Only <@{by}> can start a Box in this thread, and <@{by}> wrote nothing in this turn."
        ))));
    }
    if let Some(lost) = turn.rerun_of {
        let done = db.turn_actions(lost)?;
        if let Some(earlier) = done.iter().find(|a| a.tool == "start_box") {
            return Ok(Some(Reply::refused(format!(
                "Your last turn already started Box `{}`, so no second one starts. {}",
                earlier.box_name,
                action_line(earlier)
            ))));
        }
    }
    let boxes = db.thread_boxes(scope.channel, scope.thread_ts)?;
    let open = boxes
        .iter()
        .filter(|b| !matches!(b.state, BoxState::Gone { .. }))
        .count();
    if u32::try_from(open).map_or(true, |n| n >= scope.max_open_boxes) {
        return Ok(Some(Reply::refused(format!(
            "This thread already has {open} open Box(es), its limit. Close one first."
        ))));
    }
    Ok(None)
}

/// The repo a start uses: `asked` when it is one of the channel's repos, else the channel's
/// default. Otherwise the refusal, naming the channel's repos.
fn pick_repo(repos: &[ChannelRepo], asked: Option<&str>) -> Result<String, String> {
    let names: Vec<&str> = repos.iter().map(|r| r.repo.as_str()).collect();
    let known = if names.is_empty() {
        "This channel has no repo. Someone in this channel adds one with `/moor repo add`."
            .to_owned()
    } else {
        format!("This channel's repos: {}.", names.join(", "))
    };
    match asked {
        Some(asked) if names.contains(&asked) => Ok(asked.to_owned()),
        Some(asked) => Err(format!("This channel has no repo {asked}. {known}")),
        None => repos
            .iter()
            .find(|r| r.is_default)
            .map(|r| r.repo.clone())
            .ok_or_else(|| format!("Name a repo: this channel has no default. {known}")),
    }
}
