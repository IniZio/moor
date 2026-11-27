//! Whether a failed launch has outlasted `box.launch_timeout`.

use moor_core::seam::clock::Ts;
use moor_core::view::LaunchFailure;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use moor_core::Warning;

use crate::cx::Cx;
use crate::store::BoxRow;

/// The `box.launch_timeout` in seconds when `failure` is older than it at `now`.
pub(super) fn launch_overdue(cx: &Cx, failure: &LaunchFailure, now: Ts) -> Option<u64> {
    let limit = cx.config().box_.launch_timeout();
    let at = OffsetDateTime::parse(&failure.at, &Rfc3339).ok()?;
    let failed_ms = i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()?;
    let age = u128::try_from(now.as_millis().saturating_sub(failed_ms)).ok()?;
    (age >= limit.as_millis()).then_some(limit.as_secs())
}

/// Warns that the agent of `row` did not start in time and how to start it again: a steer
/// resumes it only when it holds a session to resume, and a new task starts one otherwise.
pub(super) fn warn_launch_timed_out(cx: &Cx, row: &BoxRow, after_secs: u64) {
    let name = row.name.as_str();
    let resumable = row
        .agent_session_id
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let again = if resumable {
        format!("write to it with `moor steer {name} TEXT` to start it again")
    } else {
        "start a new task with `moor ask --repo OWNER/NAME TEXT`".to_owned()
    };
    cx.warn(Warning {
        code: "launch_timed_out".to_owned(),
        message: format!(
            "the agent of {name} did not start within {after_secs}s, so the Box now counts as quiet and may sleep; {again}"
        ),
    });
}
