//! What a mention or thread message records before Slack gets its ack: a top-level mention
//! opens a coordinator thread with its text as the first message, and the starter's later
//! messages wait in the thread's inbox.

use super::answer::digit;
use super::Job;
use crate::command::{reply_action, strip_mentions, ReplyAction};
use crate::db::{Db, InboxItem, ItemKind, SlackThread};
use crate::socket::Mention;

/// Records one mention or thread message in a single transaction: its event id, and what
/// [`in_thread`] says for a message in a thread the front keeps. Another mention in an allowed
/// channel opens a coordinator thread. Returns what the worker does next, or nothing for a
/// redelivered event.
pub(crate) fn record(
    mention: &Mention,
    db: &Db,
    channels: &[String],
    now: i64,
) -> rusqlite::Result<Option<Job>> {
    db.atomically(|| {
        let channel = mention.channel.as_str();
        if !db.event_once(&mention.event_id, now)? || !channels.iter().any(|c| c == channel) {
            return Ok(None);
        }
        let thread = mention.thread_ts.as_deref().unwrap_or(&mention.ts);
        if let Some(kept) = db.thread(channel, thread)? {
            return in_thread(db, mention, &kept, now);
        }
        if mention.reply {
            return Ok(None);
        }
        open_coordinated(db, mention, thread, now)?;
        Ok(None)
    })
}

/// Opens a coordinator thread at `thread_ts` for `mention`, a top-level mention, inside the
/// caller's transaction: the thread row and the mention's text, with mentions stripped, as the
/// thread's first message. A bare mention opens one too, with an empty first message.
pub(super) fn open_coordinated(
    db: &Db,
    mention: &Mention,
    thread_ts: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let text = strip_mentions(&mention.text).trim().to_owned();
    let thread = SlackThread {
        channel: mention.channel.clone(),
        thread_ts: thread_ts.to_owned(),
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
    db.queue(&item, now)
}

/// Records `mention`, a message in `thread`, inside the caller's transaction: an answer to the
/// thread's one open prompt, or a message for the coordinator. Returns what the worker does
/// next.
pub(super) fn in_thread(
    db: &Db,
    mention: &Mention,
    thread: &SlackThread,
    now: i64,
) -> rusqlite::Result<Option<Job>> {
    if let Some(answer) = digit(db, mention, &thread.thread_ts)? {
        return Ok(Some(answer));
    }
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
