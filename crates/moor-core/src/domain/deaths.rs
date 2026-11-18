//! How often the places of a Box may die before moor stops resuming it on its own.

use std::time::Duration;

/// Deaths one Box may have within [`DEATH_WINDOW`]; the one that makes this many is not resumed.
pub const DEATH_CAP: usize = 3;

/// The window [`DEATH_CAP` counts over.
pub const DEATH_WINDOW: Duration = Duration::from_mins(30);

/// A Box's recent deaths once one more is counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    /// When each death inside the window happened, in unix milliseconds, oldest first; at most
    /// [`DEATH_CAP`] of the newest.
    pub kept: Vec<i64>,
    /// Whether this death is the one that reaches [`DEATH_CAP`] inside [`DEATH_WINDOW`].
    pub capped: bool,
}

/// Counts a death at `now_ms` with the `earlier` ones, dropping each that is out of the window.
#[must_use]
pub fn record(earlier: &[i64], now_ms: i64) -> Tally {
    let window = i64::try_from(DEATH_WINDOW.as_millis()).unwrap_or(i64::MAX);
    let mut kept: Vec<i64> = earlier
        .iter()
        .copied()
        .filter(|&t| t <= now_ms && now_ms - t <= window)
        .collect();
    kept.push(now_ms);
    let from = kept.len().saturating_sub(DEATH_CAP);
    kept.drain(..from);
    Tally {
        capped: kept.len() >= DEATH_CAP,
        kept,
    }
}

/// Why moor stopped resuming a Box that died `count` times in the window, the last as `why`.
#[must_use]
pub fn sentence(count: usize, why: &str) -> String {
    format!(
        "its machine stopped {count} times in {} minutes, so moor stopped resuming it: {why}",
        DEATH_WINDOW.as_secs() / 60
    )
}

#[cfg(test)]
mod tests;
