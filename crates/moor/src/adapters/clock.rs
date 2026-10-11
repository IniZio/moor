//! Wall-clock adapters: `SystemClock` (honours `MOOR_TEST_NOW`) and `FakeClock`.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moor_core::seam::clock::{Clock, Ts};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::env::Env;

/// Environment variable that pins the clock base for tests and parity runs.
pub(crate) const TEST_NOW_VAR: &str = "MOOR_TEST_NOW";

/// Failure to build a clock.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ClockError {
    /// `MOOR_TEST_NOW` held neither RFC3339 nor unix milliseconds.
    #[error("invalid MOOR_TEST_NOW `{0}`: use RFC3339 or unix milliseconds")]
    TestNow(String),
}

/// Parse unix milliseconds (all digits, optional leading `-`) or RFC3339.
pub(crate) fn parse_test_now(s: &str) -> Result<Ts, ClockError> {
    let bad = || ClockError::TestNow(s.to_owned());
    let digits = s.strip_prefix('-').unwrap_or(s);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse::<i64>().map(Ts::from_millis).map_err(|_| bad());
    }
    let at = OffsetDateTime::parse(s, &Rfc3339).map_err(|_| bad())?;
    let ms = i64::try_from(at.unix_timestamp_nanos() / 1_000_000).map_err(|_| bad())?;
    Ok(Ts::from_millis(ms))
}

/// Real clock, optionally pinned to a fixed base that advances with real time.
#[derive(Debug)]
pub(crate) struct SystemClock {
    /// Pinned base timestamp and the instant it was captured.
    base: Option<(Ts, Instant)>,
}

impl SystemClock {
    /// Build from `MOOR_TEST_NOW` (read once); unset means real time.
    pub(crate) fn from_env(env: &Env) -> Result<Self, ClockError> {
        let base = match env.get(TEST_NOW_VAR) {
            Some(raw) => Some((parse_test_now(raw)?, Instant::now())),
            None => None,
        };
        Ok(Self { base })
    }

    /// Unpinned clock reading real time.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    pub(crate) const fn real() -> Self {
        Self { base: None }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Ts {
        if let Some((ts, at)) = self.base {
            return ts.saturating_add(at.elapsed());
        }
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Ts::from_millis(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Deterministic clock that advances only when `sleep` is called.
#[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
#[derive(Debug)]
pub(crate) struct FakeClock {
    /// Total time slept so far.
    offset: Mutex<Duration>,
    /// Starting timestamp.
    start: Ts,
    /// Starting instant.
    origin: Instant,
}

impl FakeClock {
    /// Create a fake clock starting at `start`.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    pub(crate) fn new(start: Ts) -> Self {
        Self {
            offset: Mutex::new(Duration::ZERO),
            start,
            origin: Instant::now(),
        }
    }

    /// Time slept so far.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by ops from S3"))]
    fn offset(&self) -> Duration {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Ts {
        self.start.saturating_add(self.offset())
    }

    fn instant(&self) -> Instant {
        self.origin + self.offset()
    }

    fn sleep(&self, d: Duration) {
        let mut off = self.offset.lock().unwrap_or_else(PoisonError::into_inner);
        *off = off.saturating_add(d);
    }
}

#[cfg(test)]
mod tests;
