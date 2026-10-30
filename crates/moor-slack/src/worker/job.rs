//! The jobs the Socket Mode thread queues for the worker.

use moor_core::name::BoxName;

use crate::account::Account;
use crate::login::TokenCheck;
use crate::machine::MachineJob;
use crate::mcp::McpJob;
use crate::profile::ProfileJob;

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
    /// Check each pasted token and store it in the vault when its account takes it.
    CheckToken(Vec<TokenCheck>),
    /// Delete `user`'s secret for `account` from the vault and tell the user through
    /// `response_url`.
    Unlink {
        /// The Slack user who ran `/moor logout`.
        user: String,
        /// The account to unlink.
        account: Account,
        /// Where Slack takes the answer to the command.
        response_url: Option<String>,
    },
    /// Answer `/moor mcp list` or `/moor mcp remove`.
    Mcp(McpJob),
    /// Answer `/moor machine list` or `/moor machine remove`.
    Machine(MachineJob),
    /// Answer `/moor profile`.
    Profile(ProfileJob),
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
    /// Interrupt the agent of Box `name`, which its requester `by` stopped from the status
    /// message; the interrupt goes to the controller as `by`.
    Stop {
        /// The Box.
        name: BoxName,
        /// Channel id of the Box's thread.
        channel: String,
        /// Timestamp of the thread's root message.
        thread: String,
        /// Slack user id of the Box's requester.
        by: String,
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
