//! `wait`: poll a Box until it is idle or its run ends, or the timeout elapses.
//!
//! Never takes the Box lock: it only reads, and heals dead runs through the store.

use std::time::Duration;

use moor_core::decide::wait::{wait_verdict, WaitFacts, WaitVerdict};
use moor_core::seam::proc::Deadline;
use moor_core::view::run::{WaitRunView, WaitValue};
use moor_core::view::AttentionView;
use moor_core::{ErrorKind, WaitFor, WaitReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{agent_view, box_view_with_run, hide_unsupported_sleep};
use crate::ops::common::{activity, reconcile};
use crate::store::{BoxRow, RunRow};

/// Longest pause between polls.
const POLL: Duration = Duration::from_millis(250);

/// How long `wait` polls when the request sets no timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(30);

/// Shortest allowance one agent read gets, even at the timeout, so the last read can still answer.
const MIN_READ: Duration = Duration::from_secs(1);

/// Run `wait`.
pub(crate) fn run(cx: &Cx, req: &WaitReq) -> Result<WaitValue, CommandError> {
    let clock = cx.clock();
    let started = clock.instant();
    reconcile::run(cx);
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let id = store
        .latest_box_by_name(&req.box_name)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))?
        .id;
    let want = req.wait_for.unwrap_or(WaitFor::Idle);
    let timeout = req
        .timeout_secs
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
    loop {
        let read_for = allowance(timeout, clock.instant().saturating_duration_since(started));
        let row = store
            .box_row(id)?
            .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))?;
        let reading = activity::read(cx, store, &row, clock.now(), read_for)?;
        let elapsed = clock.instant().saturating_duration_since(started);
        if let Some(failure) = reading
            .launch_failure
            .as_ref()
            .filter(|_| want != WaitFor::Run)
        {
            return Err(failed(cx, &row, &reading, failure.text(), Vec::new())?);
        }
        if want != WaitFor::Run {
            let limits = store.open_limits(id)?;
            if let Some(first) = limits.first() {
                let attention = limits.iter().map(AttentionView::from).collect();
                return Err(failed(cx, &row, &reading, first.message(), attention)?);
            }
        }
        let in_flight = store.in_flight_run(id)?;
        let facts = WaitFacts {
            agent_active: reading.busy(),
            run_in_flight: in_flight.is_some(),
        };
        let verdict = wait_verdict(facts, want, elapsed, timeout);
        if verdict == WaitVerdict::Keep {
            clock.sleep(POLL.min(timeout.saturating_sub(elapsed)));
            continue;
        }
        let live = in_flight.is_some();
        let shown = match &in_flight {
            Some(r) => Some(r.clone()),
            None => store.last_run(id)?,
        };
        let mut view = box_view_with_run(&row, in_flight.as_ref(), cx.config());
        view.agent = agent_view(&reading);
        hide_unsupported_sleep(&mut view, cx);
        return Ok(WaitValue {
            boxed: view,
            timed_out: verdict == WaitVerdict::TimedOut,
            waited_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            wait_for: want,
            run: shown.as_ref().map(|r| run_view(r, live)),
        });
    }
}

/// How long one agent read may take once `elapsed` of `timeout` has passed.
fn allowance(timeout: Duration, elapsed: Duration) -> Deadline {
    let left = timeout.saturating_sub(elapsed);
    Deadline(left.clamp(MIN_READ, activity::PROBE_DEADLINE.0))
}

/// A `Failed` error carrying the Box as it stands, with `attention` filled in.
fn failed(
    cx: &Cx,
    row: &BoxRow,
    reading: &activity::Reading,
    text: impl Into<String>,
    attention: Vec<AttentionView>,
) -> Result<CommandError, CommandError> {
    let mut view = box_view_with_run(row, None, cx.config());
    view.agent = agent_view(reading);
    view.attention = attention;
    hide_unsupported_sleep(&mut view, cx);
    let shown =
        serde_json::to_value(&view).map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    Ok(CommandError::new(ErrorKind::Failed, text.into()).with_value(shown))
}

/// The run as the `wait` payload shows it.
fn run_view(r: &RunRow, in_flight: bool) -> WaitRunView {
    WaitRunView {
        id: r.id,
        argv: r.argv.clone(),
        in_flight,
        exit: r.exit,
        interrupted: r.interrupted,
    }
}

#[cfg(test)]
mod tests;
