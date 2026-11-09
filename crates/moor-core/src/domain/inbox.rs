//! A Box's inbox: each text sent to its agent waits as a row until the turn it started ends.
//!
//! A row is marked before its send and confirmed after it, so a crash between the two leaves it
//! in doubt rather than lost, and a row in doubt is sent at most once more.

use super::Illegal;
use crate::name::LaunchId;

/// A store-assigned inbox row id, in arrival order across every kind; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InboxId(i64);

impl InboxId {
    /// Wraps a raw id.
    #[must_use]
    pub const fn new(v: i64) -> Self {
        Self(v)
    }

    /// Returns the raw id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Who an inbox row is from, and whether it is a text or a key sent to answer the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InboxKind {
    /// A person steering the Box.
    Steer,
    /// A start's prompt.
    Prompt,
    /// moor telling the agent something.
    Note,
    /// A menu entry picked, its number the row's text; it answers only the screen it was sent to.
    Choice,
    /// The agent's interrupt key; like a choice, it means something only when it is sent.
    Interrupt,
}

impl InboxKind {
    /// The stored name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Prompt => "prompt",
            Self::Note => "note",
            Self::Choice => "choice",
            Self::Interrupt => "interrupt",
        }
    }

    /// The kind stored as `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Steer,
            Self::Prompt,
            Self::Note,
            Self::Choice,
            Self::Interrupt,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// Why a steer waits in the inbox instead of reaching the agent now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueCause {
    /// The Box sleeps; moor wakes it.
    Asleep,
    /// The Box waits for its node to have room to wake it.
    WaitingForRoom,
    /// The Box lost its machine or its agent, and moor starts it again.
    Relaunching,
    /// moor relaunched the agent too often to do it again on its own; `moor start --resume`.
    RelaunchCapped,
    /// Another op holds the Box, or the agent cannot take a message now.
    Busy,
}

/// Where the agent's terminal was when a row's send was confirmed, so the turn it started can be
/// told apart from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendMark {
    /// The ended-turn count the row's turn must pass; none when the send could not tell.
    pub turn_before: Option<u64>,
    /// When the status the agent showed at the send was set; none when a turn was running.
    pub status_before: Option<i64>,
}

/// How far a row got towards its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Not sent: waiting for a ready agent.
    Queued {
        /// Sends begun to agents that were gone; they count against [`MAX_SENDS`], since a gone
        /// agent's terminal may still have taken the keys.
        attempts: u8,
    },
    /// A send to `launch` began and was never confirmed: the agent may have it.
    InDoubt {
        /// The launch the send went to.
        launch: LaunchId,
        /// Sends begun, 1 or 2.
        attempts: u8,
    },
    /// The agent of `launch` has it, and its turn has not ended.
    Sent {
        /// The launch that has it.
        launch: LaunchId,
        /// When the send was confirmed, in milliseconds since the epoch.
        at: i64,
        /// The terminal at the send.
        mark: SendMark,
    },
}

/// The most sends a row gets: the first, and one more after a crash left it in doubt.
pub const MAX_SENDS: u8 = 2;

/// A row's delivery columns, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliveryColumns {
    /// Sends begun.
    pub attempts: i64,
    /// The launch last sent to.
    pub launch: Option<i64>,
    /// When the send was confirmed.
    pub sent_at: Option<i64>,
    /// The mark's ended-turn count.
    pub turn_before: Option<i64>,
    /// The mark's status time.
    pub status_before: Option<i64>,
}

impl Delivery {
    /// The delivery stored as `c`; none for a combination no transition writes.
    #[must_use]
    pub fn parse(c: DeliveryColumns) -> Option<Self> {
        let attempts = u8::try_from(c.attempts).ok().filter(|n| *n <= MAX_SENDS)?;
        match (c.launch.map(LaunchId::new), c.sent_at) {
            (None, None) if c.turn_before.is_none() && c.status_before.is_none() => {
                Some(Self::Queued { attempts })
            }
            (Some(launch), None)
                if attempts > 0 && c.turn_before.is_none() && c.status_before.is_none() =>
            {
                Some(Self::InDoubt { launch, attempts })
            }
            (Some(launch), Some(at)) if attempts > 0 => Some(Self::Sent {
                launch,
                at,
                mark: SendMark {
                    turn_before: c.turn_before.and_then(|t| u64::try_from(t).ok()),
                    status_before: c.status_before,
                },
            }),
            _ => None,
        }
    }

    /// The columns that store this delivery; a sent row keeps `sends`, the sends its row began.
    #[must_use]
    pub fn columns(self, sends: u8) -> DeliveryColumns {
        match self {
            Self::Queued { attempts } => DeliveryColumns {
                attempts: attempts.into(),
                ..DeliveryColumns::default()
            },
            Self::InDoubt { launch, attempts } => DeliveryColumns {
                attempts: attempts.into(),
                launch: Some(launch.get()),
                ..DeliveryColumns::default()
            },
            Self::Sent { launch, at, mark } => DeliveryColumns {
                attempts: sends.into(),
                launch: Some(launch.get()),
                sent_at: Some(at),
                turn_before: mark.turn_before.and_then(|t| i64::try_from(t).ok()),
                status_before: mark.status_before,
            },
        }
    }

    /// A send to `launch` begins: a queued row's, or one more to the launch a row in doubt went
    /// to, while the row has sends left.
    ///
    /// # Errors
    /// [`Illegal`] for a sent row, a row in doubt on another launch, or one out of sends.
    pub fn mark(self, launch: LaunchId) -> Result<Self, Illegal> {
        match self {
            Self::Queued { attempts } if attempts < MAX_SENDS => Ok(Self::InDoubt {
                launch,
                attempts: attempts + 1,
            }),
            Self::InDoubt {
                launch: s,
                attempts,
            } if s == launch && attempts < MAX_SENDS => Ok(Self::InDoubt {
                launch,
                attempts: attempts + 1,
            }),
            _ => Err(self.illegal("mark")),
        }
    }

    /// The send that began is confirmed at `at`, with the terminal at `mark`.
    ///
    /// # Errors
    /// [`Illegal`] unless the row is in doubt.
    pub fn confirm(self, at: i64, mark: SendMark) -> Result<Self, Illegal> {
        match self {
            Self::InDoubt { launch, .. } => Ok(Self::Sent { launch, at, mark }),
            Self::Queued { .. } | Self::Sent { .. } => Err(self.illegal("confirm")),
        }
    }

    /// The agent the send began to was gone: the row waits for another launch, and the send
    /// still counts, so no row is ever sent more than [`MAX_SENDS`] times.
    ///
    /// # Errors
    /// [`Illegal`] unless the row is in doubt.
    pub fn rollback(self) -> Result<Self, Illegal> {
        match self {
            Self::InDoubt { attempts, .. } => Ok(Self::Queued { attempts }),
            Self::Queued { .. } | Self::Sent { .. } => Err(self.illegal("rollback")),
        }
    }

    /// The agent's terminal was gone or had ended before any key was typed: undo the mark, so
    /// the refused send counts against no limit.
    ///
    /// # Errors
    /// [`Illegal`] unless the row is in doubt.
    pub fn refused(self) -> Result<Self, Illegal> {
        match self {
            Self::InDoubt { attempts, .. } => Ok(Self::Queued {
                attempts: attempts.saturating_sub(1),
            }),
            Self::Queued { .. } | Self::Sent { .. } => Err(self.illegal("refused")),
        }
    }

    /// The launch a send went to; none for a row never sent.
    #[must_use]
    pub const fn launch(self) -> Option<LaunchId> {
        match self {
            Self::Queued { .. } => None,
            Self::InDoubt { launch, .. } | Self::Sent { launch, .. } => Some(launch),
        }
    }

    /// The name of this delivery's state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued { .. } => "queued",
            Self::InDoubt { .. } => "in_doubt",
            Self::Sent { .. } => "sent",
        }
    }

    /// The error for `input`, which this delivery cannot take.
    fn illegal(self, input: &str) -> Illegal {
        Illegal {
            from: self.as_str().to_owned(),
            input: input.to_owned(),
        }
    }
}
