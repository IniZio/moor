//! `wait`: poll a Box until it is idle or its run ends, or the timeout elapses.
//!
//! Never waits for the Box's turn. Each poll settles the Box the way the controller's own reads
//! do, so `wait` reports idle only once the `idled` event is recorded. A Box still being set up
//! has no agent yet, so it is never idle, and a Box whose setup failed fails the wait.

use std::time::Duration;

use moor_core::activity::AgentState;
use moor_core::decide::wait::{wait_verdict, WaitFacts, WaitVerdict};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::clock::Ts;
use moor_core::seam::proc::Deadline;
use moor_core::view::run::{WaitRunView, WaitValue};
use moor_core::view::AttentionView;
use moor_core::{BoxId, ErrorKind, WaitFor, WaitReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{agent_view, box_view_with_run, hide_unsupported_sleep};
use crate::ops::common::{activity, reconcile};
use crate::store::{BoxRow, RunRow, Store};

/// Longest pause between polls.
const POLL: Duration = Duration::from_millis(250);

/// How long `wait` polls when the request sets no timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(30);

/// Shortest allowance one agent read gets, even at the timeout, so the last read can still answer.
const MIN_READ: Duration = Duration::from_secs(1);

/// Run `wait`.
pub(crate) fn run(cx: &Cx, req: &WaitReq) -> Result<WaitValue, CommandError> {
    run_polling(cx, req, POLL)
}

/// Like [`run`], pausing `poll` between polls.
fn run_polling(cx: &Cx, req: &WaitReq, poll: Duration) -> Result<WaitValue, CommandError> {
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
        let (row, reading) = read_settled(cx, store, id, name, read_for)?;
        let elapsed = clock.instant().saturating_duration_since(started);
        if want != WaitFor::Run {
            if let Some(err) = refusal(cx, store, &row, &reading)? {
                return Err(err);
            }
        }
        let in_flight = reconcile::run_in_flight(cx, store, id)?;
        let facts = WaitFacts {
            setting_up: matches!(
                row.lifecycle,
                BoxLifecycle::Checking | BoxLifecycle::Pending | BoxLifecycle::SettingUp
            ),
            agent_active: agent_active(store, &row, &reading, clock.now())?,
            run_in_flight: in_flight.is_some(),
        };
        let verdict = wait_verdict(facts, want, elapsed, timeout);
        if verdict == WaitVerdict::Keep {
            clock.sleep(poll.min(timeout.saturating_sub(elapsed)));
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

/// Why the Box can never go idle, as the error that ends the wait: its agent failed to launch,
/// its setup failed, or a limit it hit is still open.
fn refusal(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &activity::Reading,
) -> Result<Option<CommandError>, CommandError> {
    if let Some(failure) = &reading.launch_failure {
        return failed(cx, row, reading, failure.text(), Vec::new()).map(Some);
    }
    if row.lifecycle == BoxLifecycle::SetupFailed {
        let name = row.name.as_str();
        let text = row.state_reason.as_deref().map_or_else(
            || format!("Box {name} setup failed"),
            |why| format!("Box {name} setup failed: {why}"),
        );
        return failed(cx, row, reading, text, Vec::new()).map(Some);
    }
    let limits = store.open_limits(row.id)?;
    let Some(first) = limits.first() else {
        return Ok(None);
    };
    let attention = limits.iter().map(AttentionView::from).collect();
    failed(cx, row, reading, first.message(), attention).map(Some)
}

/// Reads Box `id`'s agent, settles the Box on that reading, and gives the Box as it then stands.
fn read_settled(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    name: &str,
    read_for: Deadline,
) -> Result<(BoxRow, activity::Reading), CommandError> {
    let row = |id| {
        store
            .box_row(id)?
            .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))
    };
    let before = row(id)?;
    let reading = activity::read(cx, store, &before, cx.clock().now(), read_for)?;
    activity::settle(cx, store, &before, &reading)?;
    Ok((row(id)?, reading))
}

/// Whether the agent counts as working: it says so, a steered turn is still open, or it is idle
/// but its Box has not gone idle yet.
fn agent_active(
    store: &Store,
    row: &BoxRow,
    reading: &activity::Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    let idle_unrecorded = reading
        .activity
        .as_ref()
        .is_some_and(|a| a.state == AgentState::Idle)
        && !row.session.is_quiet();
    Ok(reading.busy() || idle_unrecorded || activity::turn_open(store, row, reading, now)?)
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
