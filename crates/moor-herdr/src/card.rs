//! The card a Box's pane shows once it is not relaying: herdr removes a pane whose process
//! exits, so the pane stays and says what happened until the user closes it.

use moor_core::view::AttachEnd;

use crate::boxes::{Phase, Shown};

/// The key that detaches from the agent's terminal. Claude Code leaves it unbound.
pub const DETACH_KEY: &str = "ctrl+]";

/// The byte [`DETACH_KEY`] sends.
pub const DETACH_BYTE: u8 = 0x1d;

/// How the last attach went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The relay ran and ended for this reason.
    Relay(AttachEnd),
    /// The controller refused the attach, or could not be asked.
    Refused(String),
    /// The Box was not up, so nothing was attached.
    Skipped,
}

/// What the card says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// The Box is gone; its work is on its branch.
    Closed {
        /// The branch the Box pushed, when known.
        branch: Option<String>,
    },
    /// The Box is paused.
    Asleep,
    /// The user detached; the agent keeps running.
    Detached,
    /// The agent's terminal session ended while the Box stays up.
    AgentEnded,
    /// The link to the Box dropped.
    LinkDropped,
    /// The attach was refused.
    Refused(String),
}

impl Card {
    /// The card after `ended`, given the Box as the controller shows it now.
    #[must_use]
    pub fn after(ended: &Ended, now: Option<&Shown>, branch: Option<String>) -> Self {
        match now.map(|b| b.phase) {
            None | Some(Phase::Closed) => Self::Closed { branch },
            Some(Phase::Asleep) => Self::Asleep,
            Some(Phase::Live(_) | Phase::WaitingForMachine) => match ended {
                Ended::Relay(AttachEnd::Client) => Self::Detached,
                Ended::Relay(AttachEnd::Agent) => Self::AgentEnded,
                Ended::Relay(AttachEnd::Link) | Ended::Skipped => Self::LinkDropped,
                Ended::Refused(why) => Self::Refused(why.clone()),
            },
        }
    }

    /// Whether `r` attaches again from this card.
    #[must_use]
    pub const fn reattaches(&self) -> bool {
        !matches!(self, Self::Closed { .. })
    }

    /// The card's lines under the Box's `label`.
    #[must_use]
    pub fn lines(&self, label: &str) -> Vec<String> {
        let state = match self {
            Self::Closed { branch: Some(b) } => format!("closed · branch {b} · PR: gh pr view {b}"),
            Self::Closed { branch: None } => "closed".to_owned(),
            Self::Asleep => "asleep · r to attach once it wakes".to_owned(),
            Self::Detached => "detached · the agent keeps running".to_owned(),
            Self::AgentEnded => "the agent's terminal ended · the Box is still up".to_owned(),
            Self::LinkDropped => "the link to the Box dropped".to_owned(),
            Self::Refused(why) => format!("attach refused: {why}"),
        };
        let keys = if self.reattaches() {
            format!("r attach · q close this pane · {DETACH_KEY} detaches while attached")
        } else {
            "q close this pane".to_owned()
        };
        vec![label.to_owned(), String::new(), state, String::new(), keys]
    }
}

#[cfg(test)]
mod tests;
