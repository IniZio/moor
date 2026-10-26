//! What the intake gets beyond mentions and clicks: commands and modals, and what its
//! acks carry.

use std::fmt;

use moor_core::secret::Secret;
use serde_json::Value;

use crate::login::Form;

/// A `/moor` slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slash {
    /// Envelope id, used as the dedupe id.
    pub envelope_id: String,
    /// Channel the command was run in.
    pub channel: String,
    /// User who ran it.
    pub user: String,
    /// Everything after the command name.
    pub text: String,
    /// Lets the front open a modal for this user within 3 s of the command.
    pub trigger_id: Option<String>,
    /// Where the front may answer the command later, once.
    pub response_url: Option<String>,
}

/// A submitted login modal. The token is a [`Secret`] from the moment it is parsed.
pub(crate) struct TokenSubmission {
    /// Envelope id, used as the dedupe id.
    pub envelope_id: String,
    /// User who submitted the modal.
    pub user: String,
    /// The modal's view id, which the front updates with the check's outcome.
    pub view_id: String,
    /// The form the modal was opened for, as its `private_metadata` names it.
    pub account: String,
    /// Each input's block id and its pasted text, trimmed; a blank input is empty.
    pub tokens: Vec<(String, Secret)>,
}

impl fmt::Debug for TokenSubmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSubmission")
            .field("envelope_id", &self.envelope_id)
            .field("user", &self.user)
            .field("view_id", &self.view_id)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TokenSubmission {
    fn eq(&self, other: &Self) -> bool {
        self.envelope_id == other.envelope_id
            && self.user == other.user
            && self.view_id == other.view_id
            && self.account == other.account
            && self.tokens.len() == other.tokens.len()
            && self
                .tokens
                .iter()
                .zip(&other.tokens)
                .all(|(a, b)| a.0 == b.0 && a.1.digest() == b.1.digest())
    }
}

impl Eq for TokenSubmission {}

/// What the ack of a kept item carries, and what follows it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    /// The ack's `payload`, such as the text Slack shows the user who ran a command.
    pub ack: Option<Value>,
    /// What the socket thread does right after the ack.
    pub then: Option<AfterAck>,
}

/// Work the socket thread does right after an ack, because it cannot wait for the worker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AfterAck {
    /// Open the login modal with this trigger, which Slack honours for 3 s.
    OpenLogin {
        /// The command's trigger id.
        trigger_id: String,
        /// What the modal asks for.
        form: Form,
    },
    /// Open the add-server modal with this trigger.
    OpenMcp {
        /// The command's trigger id.
        trigger_id: String,
    },
    /// Open the add-machine modal with this trigger.
    OpenMachine {
        /// The command's trigger id.
        trigger_id: String,
    },
}

/// What a button press asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Press {
    /// Answer a prompt with this 1-based option number.
    Answer(u32),
    /// Stop the Box of this name: interrupt its agent.
    Stop(String),
}

/// A button press on a prompt or status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Click {
    /// Envelope id, used as the dedupe id.
    pub envelope_id: String,
    /// Channel holding the message.
    pub channel: String,
    /// User who pressed the button.
    pub user: String,
    /// Timestamp of the message carrying the button.
    pub message_ts: String,
    /// Thread the message belongs to, if known.
    pub thread_ts: Option<String>,
    /// What the button asks for.
    pub press: Press,
}
