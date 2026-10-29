//! The card a Box's pane shows once it is not relaying: herdr removes a pane whose process
//! exits, so the pane stays and says what happened until the user closes it.

use moor_core::view::AttachEnd;

use crate::boxes::{Phase, Shown};

pub use moor_core::view::progress;

pub use progress::Progress;

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

/// What a key on the card asks the pane to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Close the pane.
    Close,
    /// Wake the Box, then attach.
    Wake,
    /// Attach again.
    Attach,
    /// Tell the user why nothing happens.
    Note(&'static str),
    /// Nothing.
    Stay,
}

/// What the card says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// The Box is gone; its work is on its branch.
    Closed {
        /// The branch the Box pushed, when known.
        branch: Option<String>,
        /// The failure that left the Box down, when one did.
        why: Option<String>,
    },
    /// The Box is paused.
    Asleep,
    /// The Box is still being set up; the pane attaches once it is up.
    Starting(Progress),
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
            None | Some(Phase::Closed) => Self::Closed { branch, why: None },
            Some(Phase::Asleep) => Self::Asleep,
            Some(Phase::Starting) => Self::Starting(Progress::default()),
            Some(Phase::Live(_) | Phase::WaitingForMachine) => match ended {
                Ended::Relay(AttachEnd::Client) => Self::Detached,
                Ended::Relay(AttachEnd::Agent) => Self::AgentEnded,
                Ended::Relay(AttachEnd::Link) => Self::LinkDropped,
                Ended::Skipped => Self::Refused("the Box was not up yet; r attaches".to_owned()),
                Ended::Refused(why) => Self::Refused(why.clone()),
            },
        }
    }

    /// What pressing `byte` on this card does.
    #[must_use]
    pub const fn key(&self, byte: u8) -> Choice {
        match (byte, self) {
            (b'q' | b'Q', _) => Choice::Close,
            (b'r' | b'R' | b'\r', Self::Closed { .. }) => {
                Choice::Note("closed: there is nothing to attach")
            }
            (b'r' | b'R' | b'\r', Self::Starting(_)) => Choice::Stay,
            (b'r' | b'R' | b'\r', Self::Asleep) => Choice::Wake,
            (b'r' | b'R' | b'\r', _) => Choice::Attach,
            _ => Choice::Stay,
        }
    }

    /// Whether `r` attaches again from this card.
    #[must_use]
    pub const fn reattaches(&self) -> bool {
        !matches!(self, Self::Closed { .. } | Self::Starting(_))
    }

    /// The card's lines under the Box's `label`.
    #[must_use]
    pub fn lines(&self, label: &str) -> Vec<String> {
        let state = match self {
            Self::Closed { why: Some(why), .. } => why.clone(),
            Self::Closed {
                branch: Some(b), ..
            } => format!("closed · branch {b} · PR: gh pr view {b}"),
            Self::Closed { branch: None, .. } => "closed".to_owned(),
            Self::Asleep => "asleep · r wakes it and attaches".to_owned(),
            Self::Starting(p) => p.elapsed.map_or_else(
                || "starting · attaches when it's up".to_owned(),
                |secs| {
                    format!(
                        "starting · {} · attaches when it's up",
                        progress::span(secs)
                    )
                },
            ),
            Self::Detached => "detached · the agent keeps running".to_owned(),
            Self::AgentEnded => "the agent's terminal ended · the Box is still up".to_owned(),
            Self::LinkDropped => "the link to the Box dropped".to_owned(),
            Self::Refused(why) => format!("attach refused: {why}"),
        };
        let keys = if matches!(self, Self::Asleep) {
            "r wake and attach · q close this pane".to_owned()
        } else if self.reattaches() {
            format!("r attach · q close this pane · {DETACH_KEY} detaches while attached")
        } else {
            "q close this pane".to_owned()
        };
        let mut lines = vec![label.to_owned(), String::new(), state, String::new()];
        if let Self::Starting(p) = self {
            lines.extend(p.steps.iter().cloned());
            lines.extend((!p.steps.is_empty()).then(String::new));
        }
        lines.push(keys);
        lines
    }
}

#[cfg(test)]
mod tests;
