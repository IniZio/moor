//! Wall-clock adapter: `SystemClock` (honours `MOOR_TEST_NOW`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moor_core::seam::clock::{Clock, Ts};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::env::Env;

/// Environment variable that pins the clock base for tests.
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
    #[cfg_attr(not(test), expect(dead_code, reason = "used by wire"))]
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

#[cfg(test)]
mod tests;
