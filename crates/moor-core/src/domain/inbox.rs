//! A Box's inbox: each text sent to its agent waits as a row until the turn it started ends.
//!
//! A row is marked before its send and confirmed after it, so a crash between the two leaves it
//! in doubt rather than lost, and a row in doubt is sent at most once more.

use super::Illegal;
use crate::name::SessionId;

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

/// Who an inbox row is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InboxKind {
    /// A person steering the Box.
    Steer,
    /// A start's prompt.
    Prompt,
    /// moor telling the agent something.
    Note,
}

impl InboxKind {
    /// The stored name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Prompt => "prompt",
            Self::Note => "note",
        }
    }

    /// The kind stored as `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Steer, Self::Prompt, Self::Note]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
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
    Queued,
    /// A send to launch `session` began and was never confirmed: the agent may have it.
    InDoubt {
        /// The launch the send went to.
        session: SessionId,
        /// Sends begun, 1 or 2.
        attempts: u8,
    },
    /// The agent of launch `session` has it, and its turn has not ended.
    Sent {
        /// The launch that has it.
        session: SessionId,
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
    pub session: Option<i64>,
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
        match (c.session.map(SessionId::new), c.sent_at) {
            (None, None)
                if attempts == 0 && c.turn_before.is_none() && c.status_before.is_none() =>
            {
                Some(Self::Queued)
            }
            (Some(session), None)
                if attempts > 0 && c.turn_before.is_none() && c.status_before.is_none() =>
            {
                Some(Self::InDoubt { session, attempts })
            }
            (Some(session), Some(at)) if attempts > 0 => Some(Self::Sent {
                session,
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
            Self::Queued => DeliveryColumns::default(),
            Self::InDoubt { session, attempts } => DeliveryColumns {
                attempts: attempts.into(),
                session: Some(session.get()),
                ..DeliveryColumns::default()
            },
            Self::Sent { session, at, mark } => DeliveryColumns {
                attempts: sends.into(),
                session: Some(session.get()),
                sent_at: Some(at),
                turn_before: mark.turn_before.and_then(|t| i64::try_from(t).ok()),
                status_before: mark.status_before,
            },
        }
    }

    /// A send to launch `session` begins: a queued row's first, or one more to the launch a row
    /// in doubt went to, while it has sends left.
    ///
    /// # Errors
    /// [`Illegal`] for a sent row, a row in doubt on another launch, or one out of sends.
    pub fn mark(self, session: SessionId) -> Result<Self, Illegal> {
        match self {
            Self::Queued => Ok(Self::InDoubt {
                session,
                attempts: 1,
            }),
            Self::InDoubt {
                session: s,
                attempts,
            } if s == session && attempts < MAX_SENDS => Ok(Self::InDoubt {
                session,
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
            Self::InDoubt { session, .. } => Ok(Self::Sent { session, at, mark }),
            Self::Queued | Self::Sent { .. } => Err(self.illegal("confirm")),
        }
    }

    /// The send that began never reached the agent, which was gone: undo its mark.
    ///
    /// # Errors
    /// [`Illegal`] unless the row is in doubt.
    pub fn rollback(self) -> Result<Self, Illegal> {
        match self {
            Self::InDoubt { attempts: 1, .. } => Ok(Self::Queued),
            Self::InDoubt { session, attempts } => Ok(Self::InDoubt {
                session,
                attempts: attempts - 1,
            }),
            Self::Queued | Self::Sent { .. } => Err(self.illegal("rollback")),
        }
    }

    /// The name of this delivery's state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
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
