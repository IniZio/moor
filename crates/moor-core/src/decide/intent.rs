//! Dead in-flight intents.

use std::time::Duration;

use crate::domain::decision::Intent;
use crate::name::BoxHandle;
use crate::seam::clock::Ts;

/// How long a Provisioning intent may sit without a Box handle before its owner counts as gone.
pub const PROVISIONING_GRACE: Duration = Duration::from_secs(120);

/// Time from `since` to `now`; zero if `now` is earlier.
#[must_use]
pub fn age(since: Ts, now: Ts) -> Duration {
    let ms = now.as_millis().saturating_sub(since.as_millis());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

/// True when a Provisioning intent's process is dead, no Box handle was recorded, and the
/// intent is older than [`PROVISIONING_GRACE`].
#[must_use]
pub fn provisioning_dead(
    intent: &Intent,
    alive: bool,
    handle: Option<&BoxHandle>,
    age: Duration,
) -> bool {
    matches!(intent, Intent::Provisioning { .. })
        && !alive
        && handle.is_none()
        && age > PROVISIONING_GRACE
}

#[cfg(test)]
mod tests;
