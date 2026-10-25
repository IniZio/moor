//! Whether a failed launch has outlasted `box.launch_timeout`.

use moor_core::seam::clock::Ts;
use moor_core::view::LaunchFailure;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::cx::Cx;

/// The `box.launch_timeout` in seconds when `failure` is older than it at `now`.
pub(super) fn launch_overdue(cx: &Cx, failure: &LaunchFailure, now: Ts) -> Option<u64> {
    let limit = cx.config().box_.launch_timeout();
    let at = OffsetDateTime::parse(&failure.at, &Rfc3339).ok()?;
    let failed_ms = i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()?;
    let age = u128::try_from(now.as_millis().saturating_sub(failed_ms)).ok()?;
    (age >= limit.as_millis()).then_some(limit.as_secs())
}
