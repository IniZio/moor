//! The jobs the Socket Mode thread queues for the worker.

use crate::login::TokenCheck;

/// What the worker does for one recorded mention or command.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Job {
    /// Post `text` in a thread.
    Reply {
        /// Channel id of the thread.
        channel: String,
        /// Timestamp of the thread's root message.
        thread: String,
        /// What to post.
        text: String,
    },
    /// Check a pasted Claude token and store it when Claude takes it.
    CheckToken(TokenCheck),
    /// Show `text` to `user` only, in a thread.
    Ephemeral {
        /// Channel id of the thread.
        channel: String,
        /// The only user who sees it.
        user: String,
        /// Timestamp of the thread's root message.
        thread: String,
        /// What to show.
        text: String,
    },
    /// Answer prompt `prompt` with option `choice`, which `by` picked.
    Answer {
        /// The prompt's id.
        prompt: i64,
        /// The option, numbered from 1.
        choice: u32,
        /// Slack user id of the person who answered.
        by: String,
    },
}
