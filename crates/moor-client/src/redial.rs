//! Redialling a controller while a request provably did not run.

use std::time::Duration;

/// How long and how often a client dials again a controller that is not listening or that
/// answered `restarting`. Only those two failures prove a request never ran, so only they
/// are retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Redial {
    /// Longest total pause across one run of failed dials before the last failure is
    /// returned. Time spent dialling does not count.
    pub budget: Duration,
    /// Pause before the first redial; each later pause doubles, up to `max_pause`.
    pub first_pause: Duration,
    /// Longest pause between two dials.
    pub max_pause: Duration,
}

impl Redial {
    /// No redial: the first failure is returned as it is.
    pub const NEVER: Self = Self {
        budget: Duration::ZERO,
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

    /// A fresh run of pauses.
    const fn backoff(&self) -> Backoff {
        Backoff {
            policy: *self,
            left: self.budget,
            next: self.first_pause,
            paused: false,
        }
    }
}

impl Default for Redial {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(60),
            first_pause: Duration::from_millis(100),
            max_pause: Duration::from_secs(2),
        }
    }
}

/// The pauses of one run of failed dials.
#[derive(Debug, Clone)]
struct Backoff {
    /// Budget and pause bounds.
    policy: Redial,
    /// Budget not yet handed out as pauses.
    left: Duration,
    /// The pause the next call hands out.
    next: Duration,
    /// Set once a pause was handed out.
    paused: bool,
}

impl Backoff {
    /// The pause before the next dial, or `None` once it would end past the budget.
    fn pause(&mut self) -> Option<Duration> {
        let pause = self.next.min(self.left);
        if pause.is_zero() {
            return None;
        }
        self.left -= pause;
        self.next = (self.next * 2).min(self.policy.max_pause);
        self.paused = true;
        Some(pause)
    }

    /// True once a pause was handed out, so a failure now means the budget ran out.
    const fn started(&self) -> bool {
        self.paused
    }
}

/// How one dial ended.
#[derive(Debug)]
pub enum Attempt<T, E> {
    /// The controller answered.
    Done(T),
    /// The request never ran: no controller listened, or it answered `restarting`.
    Unrun(E),
    /// Anything else, including a connection cut after the request was sent.
    Failed(E),
}

/// Why redialling gave up.
#[derive(Debug)]
pub enum GaveUp<E> {
    /// A failure that is never retried, or the first unrun one with a zero budget.
    Failed(E),
    /// Every dial within the budget left the request unrun; `last` is the final failure.
    Budget {
        /// The budget that ran out.
        budget: Duration,
        /// The final failure.
        last: E,
    },
}

/// Runs `attempt` until it is done or fails, sleeping each pause through `sleep` between
/// unrun dials. `sleep` returns false to stop early, which returns the last failure.
///
/// # Errors
/// Returns [`GaveUp`] with the failure that ended the run.
pub fn redial<T, E>(
    policy: &Redial,
    mut attempt: impl FnMut() -> Attempt<T, E>,
    mut sleep: impl FnMut(Duration, &E) -> bool,
) -> Result<T, GaveUp<E>> {
    let mut backoff = policy.backoff();
    loop {
        match attempt() {
            Attempt::Done(value) => return Ok(value),
            Attempt::Failed(e) => return Err(GaveUp::Failed(e)),
            Attempt::Unrun(e) => match backoff.pause() {
                Some(pause) if sleep(pause, &e) => {}
                None if backoff.started() => {
                    return Err(GaveUp::Budget {
                        budget: policy.budget,
                        last: e,
                    })
                }
                _ => return Err(GaveUp::Failed(e)),
            },
        }
    }
}
