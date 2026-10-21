//! What a mention or thread message records before Slack gets its ack: a start opens a
//! thread with its first Box, the starter's messages wait in the thread's inbox, and `close`
//! marks the thread's Box `closing`.

use moor_core::name::BoxName;

use super::Job;
use crate::command::{close_target, reply_action, ReplyAction};
use crate::db::{BoxState, Db, InboxItem, ItemKind, SlackBox, SlackThread};
use crate::socket::Mention;

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
    let item = |kind: ItemKind, text: String| InboxItem {
        channel: thread.channel.clone(),
        thread_ts: thread.thread_ts.clone(),
        kind,
        author: mention.user.clone(),
        text,
        source: mention.event_id.clone(),
    };
    Ok(
        match reply_action(&thread.owner, &mention.user, &mention.text) {
            ReplyAction::Ignore => None,
            ReplyAction::Refuse(text) => Some(say(text)),
            ReplyAction::Queue(text) => {
                db.queue(&item(ItemKind::Message, text), now)?;
                None
            }
            ReplyAction::Close => {
                let boxes = db.thread_boxes(&thread.channel, &thread.thread_ts)?;
                match close_target(&boxes, &mention.user) {
                    Err(text) => Some(say(text)),
                    Ok(target) => {
                        db.queue(&item(ItemKind::Close, "close".to_owned()), now)?;
                        db.mark_closing(&target.box_name)?;
                        Some(Job::Close(closing(target)))
                    }
                }
            }
        },
    )
}

/// `target`, a bound Box, as it stands once marked `closing`.
fn closing(target: &SlackBox) -> SlackBox {
    let state = match &target.state {
        BoxState::Bound { box_id, branch } => BoxState::Closing {
            box_id: *box_id,
            branch: branch.clone(),
        },
        other => other.clone(),
    };
    SlackBox {
        state,
        ..target.clone()
    }
}
