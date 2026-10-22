//! Redialling a controller while a request provably did not run.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moor_core::seam::clock::{Clock, Ts};

/// How long and how often a client dials again a controller that refused the connection or
/// proved it is restarting. Only those failures prove a request never ran, so only they are
/// retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Redial {
    /// Longest wall time, dials included, that one run of failed dials lasts before the last
    /// failure is returned.
    pub budget: Duration,
    /// Longest wall time a run of refused connects lasts, within `budget`. A refused connect
    /// proves only that nothing listens on the path, not that a controller is coming back.
    pub refused: Duration,
    /// Pause before the first redial; each later pause doubles, up to `max_pause`.
    pub first_pause: Duration,
    /// Longest pause between two dials.
    pub max_pause: Duration,
}

impl Redial {
    /// No redial: the first failure is returned as it is.
    pub const NEVER: Self = Self {
        budget: Duration::ZERO,
        refused: Duration::ZERO,
        first_pause: Duration::ZERO,
        max_pause: Duration::ZERO,
    };

    /// The default pauses with `budget` in place of the default budget.
    #[must_use]
    pub fn within(budget: Duration) -> Self {
        Self {
            budget,
            ..Self::default()
        }
    }
}

impl Default for Redial {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(60),
            refused: Duration::from_secs(5),
            first_pause: Duration::from_millis(100),
            max_pause: Duration::from_secs(2),
        }
    }
}

/// Exit code of `moor controller attach` when the controller socket refused its connect. A
/// client over ssh dials again on it, as it would on a refused local connect.
pub const ATTACH_REFUSED_EXIT: u8 = 75;

/// How one dial ended.
#[derive(Debug)]
pub enum Attempt<T, E> {
    /// The controller answered.
    Done(T),
    /// The connect was refused on an existing socket path: a stale socket from a controller
    /// that died, or one between unlink and bind. Redialled for at most [`Redial::refused`].
    Refused(E),
    /// The request never ran and the controller is restarting: it answered `restarting`, or
    /// the connection ended before its hello. Redialled within the whole [`Redial::budget`].
    Unrun(E),
    /// Anything else, including a missing socket path and a connection cut after the request
    /// was sent.
    Failed(E),
}

impl<T, E> Attempt<T, E> {
    /// A connect that failed with `kind`, reported as `e`: a refused one is retried; any
    /// other, a missing socket path included, fails at once, since no controller is set up.
    pub fn connect_failed(kind: std::io::ErrorKind, e: E) -> Self {
        if kind == std::io::ErrorKind::ConnectionRefused {
            Self::Refused(e)
        } else {
            Self::Failed(e)
        }
    }

    /// Continues a dial that got as far as `T` with `next`; any other outcome stays as it is.
    pub fn and_then<U>(self, next: impl FnOnce(T) -> Attempt<U, E>) -> Attempt<U, E> {
        match self {
            Self::Done(value) => next(value),
            Self::Refused(e) => Attempt::Refused(e),
            Self::Unrun(e) => Attempt::Unrun(e),
            Self::Failed(e) => Attempt::Failed(e),
        }
    }
}

/// Why redialling gave up.
#[derive(Debug)]
pub enum GaveUp<E> {
    /// A failure that is never retried, or the first retryable one with a zero budget.
    Failed(E),
    /// Every dial within the budget left the request unrun; `last` is the final failure.
    Budget {
        /// The budget that ran out: [`Redial::refused`] for a run of refused connects.
        budget: Duration,
        /// The final failure.
        last: E,
    },
}

/// Runs `attempt` until it is done or fails, sleeping each pause through `sleep` between
/// retryable dials. Time is read from `clock`, so dials count against the budget too.
///
/// # Errors
/// Returns [`GaveUp`] with the failure that ended the run.
pub fn redial<T, E>(
    policy: &Redial,
    clock: &dyn Clock,
    mut attempt: impl FnMut() -> Attempt<T, E>,
    mut sleep: impl FnMut(Duration, &E),
) -> Result<T, GaveUp<E>> {
    let began = clock.instant();
    let mut refused_since: Option<Instant> = None;
    let mut next = policy.first_pause;
    let mut paused = false;
    loop {
        let tried = clock.instant();
        let (e, limit) = match attempt() {
            Attempt::Done(value) => return Ok(value),
            Attempt::Failed(e) => return Err(GaveUp::Failed(e)),
            Attempt::Refused(e) => {
                let since = *refused_since.get_or_insert(tried);
                let spent = clock.instant().saturating_duration_since(since);
                let left = policy.refused.saturating_sub(spent);
                (e, Some(left))
            }
            Attempt::Unrun(e) => {
                refused_since = None;
                (e, None)
            }
        };
        let spent = clock.instant().saturating_duration_since(began);
        let left = policy.budget.saturating_sub(spent);
        let (left, budget) = match limit {
            Some(refused) if refused < left => (refused, policy.refused),
            _ => (left, policy.budget),
        };
        let pause = next.min(left);
        if pause.is_zero() {
            return Err(if paused {
                GaveUp::Budget { budget, last: e }
            } else {
                GaveUp::Failed(e)
            });
        }
        sleep(pause, &e);
        paused = true;
        next = (next * 2).min(policy.max_pause);
    }
}

/// The real clock that redial runs read.
#[derive(Debug, Clone, Copy, Default)]
pub struct WallClock;

impl Clock for WallClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "the client's clock adapter is the one place it reads the clock"
    )]
    fn now(&self) -> Ts {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Ts::from_millis(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the client's clock adapter is the one place it reads the clock"
    )]
    fn instant(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}
