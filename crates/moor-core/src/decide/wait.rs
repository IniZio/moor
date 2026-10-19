//! Wait verdicts.

use std::time::Duration;

use crate::request::WaitFor;

/// What `wait` can observe about a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitFacts {
    /// An agent is currently working.
    pub agent_active: bool,
    /// A run is in flight.
    pub run_in_flight: bool,
}

/// Outcome of one wait poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitVerdict {
    /// The condition holds.
    Done,
    /// Keep polling.
    Keep,
    /// The timeout elapsed first.
    TimedOut,
}

/// Decides one poll: the condition wins over the timeout, and the timeout fires once
/// `elapsed >= timeout`.
#[must_use]
pub const fn wait_verdict(
    facts: WaitFacts,
    want: WaitFor,
    elapsed: Duration,
    timeout: Duration,
) -> WaitVerdict {
    let met = match want {
        WaitFor::Idle | WaitFor::Done | WaitFor::Input => {
            !facts.agent_active && !facts.run_in_flight
        }
        WaitFor::Run => !facts.run_in_flight,
    };
    if met {
        WaitVerdict::Done
    } else if elapsed.as_nanos() >= timeout.as_nanos() {
        WaitVerdict::TimedOut
    } else {
        WaitVerdict::Keep
    }
}

#[cfg(test)]
mod tests;
