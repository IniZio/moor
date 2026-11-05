//! A workflow's schedule and run timeout: the parts that read the cron library.

use std::fmt;
use std::time::Duration;

use croner::parser::{CronParser, Seconds};
use croner::Cron;
use jiff::tz::TimeZone;

use super::WorkflowError;
use crate::seam::clock::Ts;

/// The shortest gap between two firings a schedule may have, unless the validator is given
/// another floor.
pub const MIN_FIRING_INTERVAL: Duration = Duration::from_secs(3600);

/// From when [`WorkflowSchedule`] looks for firings closer than the floor, 2024-01-01 UTC.
const FLOOR_CHECK_FROM_MS: i64 = 1_704_067_200_000;

/// How far past [`FLOOR_CHECK_FROM_MS`] it looks: two years, which holds a leap day and both
/// clock changes of each zone.
const FLOOR_CHECK_SPAN_MS: i64 = 2 * 366 * 86_400_000;

/// How long a started run may take before it is expired, in minutes: 1 to 720.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeoutMinutes(u32);

impl TimeoutMinutes {
    /// The timeout a workflow gets when its owner names none: three hours.
    pub const DEFAULT: Self = Self(180);

    /// Validates a timeout of 1 to 720 minutes.
    ///
    /// # Errors
    /// [`WorkflowError::Timeout`] outside that range.
    pub fn new(minutes: u32) -> Result<Self, WorkflowError> {
        if (1..=720).contains(&minutes) {
            Ok(Self(minutes))
        } else {
            Err(WorkflowError::Timeout(minutes))
        }
    }

    /// The timeout in minutes.
    #[must_use]
    pub const fn minutes(self) -> u32 {
        self.0
    }

    /// The timeout in milliseconds.
    #[must_use]
    pub fn millis(self) -> i64 {
        i64::from(self.0) * 60_000
    }
}

impl Default for TimeoutMinutes {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// When a workflow fires: a five-field cron expression read in an IANA time zone.
#[derive(Clone)]
pub struct WorkflowSchedule {
    /// The parsed expression.
    cron: Cron,
    /// The expression as written.
    expr: String,
    /// The parsed zone.
    zone: TimeZone,
    /// The zone as written.
    zone_name: String,
}

impl WorkflowSchedule {
    /// Parses a five-field cron expression and an IANA zone name.
    ///
    /// # Errors
    /// [`WorkflowError::Cron`] when `expr` is not five fields of valid cron, and
    /// [`WorkflowError::Zone`] when `zone` is not an IANA name.
    pub fn parse(expr: &str, zone: &str) -> Result<Self, WorkflowError> {
        Self::parse_with_min_interval(expr, zone, MIN_FIRING_INTERVAL)
    }

    /// Like [`Self::parse`], and rejects a schedule whose consecutive firings are closer than
    /// `floor` anywhere in the two years from 2024-01-01 UTC, judged on the firing times the cron
    /// library yields, not on the expression. A zero `floor` accepts any schedule.
    ///
    /// # Errors
    /// As [`Self::parse`], and [`WorkflowError::TooFrequent`].
    pub fn parse_with_min_interval(
        expr: &str,
        zone: &str,
        floor: Duration,
    ) -> Result<Self, WorkflowError> {
        let expr = expr.trim();
        let bad = |reason: &str| WorkflowError::Cron {
            expr: expr.to_owned(),
            reason: reason.to_owned(),
        };
        if expr.split_whitespace().count() != 5 {
            return Err(bad(
                "use five fields: minute hour day-of-month month day-of-week",
            ));
        }
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .build()
            .parse(expr)
            .map_err(|e| bad(&e.to_string()))?;
        let tz = TimeZone::get(zone).map_err(|_| WorkflowError::Zone(zone.to_owned()))?;
        let schedule = Self {
            cron,
            expr: expr.to_owned(),
            zone: tz,
            zone_name: zone.to_owned(),
        };
        if schedule.fires_within(floor) {
            return Err(WorkflowError::TooFrequent {
                expr: expr.to_owned(),
                floor_minutes: floor.as_secs().div_ceil(60),
            });
        }
        Ok(schedule)
    }

    /// Whether two consecutive firings in the checked span are less than `floor` apart.
    fn fires_within(&self, floor: Duration) -> bool {
        if floor.is_zero() {
            return false;
        }
        let floor_ms = i64::try_from(floor.as_millis()).unwrap_or(i64::MAX);
        let end = FLOOR_CHECK_FROM_MS + FLOOR_CHECK_SPAN_MS;
        let mut prev = self.first_after(Ts::from_millis(FLOOR_CHECK_FROM_MS - 1));
        while let Some(at) = prev.filter(|t| t.as_millis() < end) {
            let next = self.first_after(at);
            if next.is_some_and(|n| n.as_millis() - at.as_millis() < floor_ms) {
                return true;
            }
            prev = next;
        }
        false
    }

    /// The cron expression.
    #[must_use]
    pub fn expr(&self) -> &str {
        &self.expr
    }

    /// The IANA zone name.
    #[must_use]
    pub fn zone(&self) -> &str {
        &self.zone_name
    }

    /// The latest firing time at or before `at`, if the schedule has one.
    #[must_use]
    pub fn latest_at_or_before(&self, at: Ts) -> Option<Ts> {
        let local = jiff::Timestamp::from_millisecond(at.as_millis())
            .ok()?
            .to_zoned(self.zone.clone());
        let due = self.cron.find_previous_occurrence(&local, true).ok()?;
        Some(Ts::from_millis(due.timestamp().as_millisecond()))
    }

    /// The first firing time after `at`, if the schedule has one.
    #[must_use]
    pub fn first_after(&self, at: Ts) -> Option<Ts> {
        let local = jiff::Timestamp::from_millisecond(at.as_millis())
            .ok()?
            .to_zoned(self.zone.clone());
        let due = self.cron.find_next_occurrence(&local, false).ok()?;
        Some(Ts::from_millis(due.timestamp().as_millisecond()))
    }
}

impl PartialEq for WorkflowSchedule {
    fn eq(&self, other: &Self) -> bool {
        self.expr == other.expr && self.zone_name == other.zone_name
    }
}

impl Eq for WorkflowSchedule {}

impl fmt::Debug for WorkflowSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkflowSchedule({} {})", self.expr, self.zone_name)
    }
}
