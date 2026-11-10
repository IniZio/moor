//! Dead effects in flight.

use std::time::Duration;

use crate::domain::decision::Effect;
use crate::name::BoxHandle;
use crate::seam::clock::Ts;

/// How long an acquire may sit without a Box handle before its start counts as gone.
pub const PROVISIONING_GRACE: Duration = Duration::from_secs(120);

/// Time from `since` to `now`; zero if `now` is earlier.
#[must_use]
pub fn age(since: Ts, now: Ts) -> Duration {
    let ms = now.as_millis().saturating_sub(since.as_millis());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

/// True when no op on the Box is in flight (`busy` is false), no Box handle was recorded, and an
/// acquire in flight is older than [`PROVISIONING_GRACE`].
#[must_use]
pub fn provisioning_dead(
    effect: &Effect,
    busy: bool,
    handle: Option<&BoxHandle>,
    age: Duration,
) -> bool {
    matches!(effect, Effect::Acquire) && !busy && handle.is_none() && age > PROVISIONING_GRACE
}

/// True when the process running a setup in flight is dead. A live one is never dead, so there
/// is no grace.
#[must_use]
pub const fn setup_dead(effect: &Effect, alive: bool) -> bool {
    matches!(effect, Effect::Setup) && !alive
}

#[cfg(test)]
mod tests;
