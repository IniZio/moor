//! What a mention or thread message records before Slack gets its ack: a start opens a
//! thread with its first Box, and the starter's messages wait in the thread's inbox.

use moor_core::name::BoxName;

use super::Job;
use crate::command::{decide, is_start, reply_action, Action, Allow, ReplyAction};
use crate::db::{BoxState, Db, InboxItem, ItemKind, SlackBox, SlackThread};
use crate::socket::Mention;

/// Records one mention or thread message in a single transaction: its event id and, for an
/// allowed start, what [`open`] writes. A message in a thread the front keeps is
/// recorded as [`in_thread`] says, unless it is a start command. With `coordinated`,
/// another top-level mention from an allowed user opens a coordinator thread. Returns what the
/// worker does next, or nothing for a redelivered event.
pub(crate) fn record(
    mention: &Mention,
    db: &Db,
    allow: &Allow<'_>,
    coordinated: bool,
    now: i64,
) -> rusqlite::Result<Option<Job>> {
    db.atomically(|| {
        let channel = mention.channel.as_str();
        if !db.event_once(&mention.event_id, now)? || !allow.channels.iter().any(|c| c == channel) {
            return Ok(None);
        }
        let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
        match db.thread(channel, thread)? {
            Some(kept) if mention.reply || !is_start(&mention.text) => {
                return in_thread(db, mention, &kept, now);
            }
            None if mention.reply => return Ok(None),
            None if coordinated
                && !is_start(&mention.text)
                && allow.users.contains(&mention.user)
                && open_coordinated(db, mention, thread, now)? =>
            {
                return Ok(None);
            }
            _ => {}
        }
        let existing = db.thread_boxes(channel, thread)?;
        let action = decide(
            allow,
            channel,
            &mention.user,
            &mention.text,
            thread,
            existing.last(),
        );
        Ok(match action {
            Action::Ignore => None,
            Action::Reply(text) => Some(Job::Reply {
                channel: channel.to_owned(),
                thread: thread.to_owned(),
                text,
            }),
            Action::Start { name, repo, goal } => {
                let opened = SlackThread {
                    channel: channel.to_owned(),
                    thread_ts: thread.to_owned(),
                    repo,
                    owner: mention.user.clone(),
                };
                Some(Job::Start(open(db, opened, name, goal, now)?))
            }
        })
    })
}

/// Records `thread` with its repo and owner and the thread's `starting` Box `name` with its
/// goal and the owner as requester, inside the caller's transaction. Returns the Box.
pub(super) fn open(
    db: &Db,
    thread: SlackThread,
    name: BoxName,
    goal: String,
    now: i64,
) -> rusqlite::Result<SlackBox> {
    db.insert_thread(&thread, now)?;
    let row = SlackBox {
        channel: thread.channel,
        thread_ts: thread.thread_ts,
        box_name: name,
        repo: thread.repo,
        goal,
        requested_by: thread.owner,
        state: BoxState::Starting,
    };
    db.insert_starting(&row, now)?;
    Ok(row)
}

/// Opens a coordinator thread at `thread_ts` for `mention`, a top-level mention that is not a
/// start command, inside the caller's transaction: the thread row, with the channel's default
/// repo when it has one, and the mention's text as the thread's first message. Returns false,
/// and writes nothing, when the text is empty once mentions are stripped.
pub(super) fn open_coordinated(
    db: &Db,
    mention: &Mention,
    thread_ts: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let ReplyAction::Queue(text) = reply_action(&mention.user, &mention.user, &mention.text) else {
        return Ok(false);
    };
    let repos = db.channel_repos(&mention.channel)?;
    let repo = repos.into_iter().find(|r| r.is_default).map(|r| r.repo);
    let thread = SlackThread {
        channel: mention.channel.clone(),
        thread_ts: thread_ts.to_owned(),
        repo: repo.unwrap_or_default(),
        owner: mention.user.clone(),
    };
    db.insert_thread(&thread, now)?;
    let item = InboxItem {
        channel: thread.channel,
        thread_ts: thread.thread_ts,
        kind: ItemKind::Message,
        author: thread.owner,
        text,
        source: mention.event_id.clone(),
    };
    db.queue(&item, now)?;
    Ok(true)
}

/// Records `mention`, a message in `thread`, inside the caller's transaction. Returns what the
/// worker does next.
pub(super) fn in_thread(
    db: &Db,
    mention: &Mention,
    thread: &SlackThread,
    now: i64,
) -> rusqlite::Result<Option<Job>> {
    let say = |text: String| Job::Reply {
        channel: thread.channel.clone(),
        thread: thread.thread_ts.clone(),
        text,
    };
    let item = |text: String| InboxItem {
        channel: thread.channel.clone(),
        thread_ts: thread.thread_ts.clone(),
        kind: ItemKind::Message,
        author: mention.user.clone(),
        text,
        source: mention.event_id.clone(),
    };
    Ok(
        match reply_action(&thread.owner, &mention.user, &mention.text) {
            ReplyAction::Ignore => None,
            ReplyAction::Refuse(text) => Some(say(text)),
            ReplyAction::Queue(text) => {
                db.queue(&item(text), now)?;
                None
            }
        },
    )
}
