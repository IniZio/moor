//! Relaunching an exited agent so a steer can reach it, at most once per steer.

use std::time::Duration;

use moor_core::{BoxId, ErrorDetail, ErrorKind};

use super::Gone;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::link::{self, Fate};
use crate::ops::start::revive_for_steer;
use crate::store::Store;

/// An agent that is gone again this soon after a relaunch is not relaunched again.
const RELAUNCH_GUARD: Duration = Duration::from_secs(60);

/// Recent events searched for the last relaunch.
const RECENT_EVENTS: usize = 50;

/// Relaunches the gone agent of `target`, resuming its stored agent session.
///
/// An agent that another op relaunched while this one waited for the Box lock is left to run.
///
/// # Errors
/// The gone agent's refusal when there is nothing to resume, a failure naming why the
/// relaunched agent did not start, or [`ended_again`] when the agent was relaunched less than
/// `RELAUNCH_GUARD` ago. Every error leaves the Session ended.
pub(super) fn once(
    cx: &Cx,
    store: &Store,
    (id, name): (BoxId, &str),
    mut gone: Gone,
) -> Result<(), CommandError> {
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "steer", boxlock::WAIT)?;
    let live = store.live_session(id)?;
    let live_id = live.as_ref().map(|l| l.id);
    if live_id.is_some() && live_id != gone.launch.as_ref().map(|l| l.id) {
        return Ok(());
    }
    if let (Some(launch), Some(row)) = (live, store.box_row(id)?) {
        match link::recover(cx, store, &row, &launch, gone.code)? {
            Fate::Reattached => return Ok(()),
            Fate::Ended(exit) => gone.code = exit,
        }
    }
    if relaunched_recently(cx, store, id)? {
        return Err(ended_again(cx, store, (id, name), &gone)?);
    }
    let view = revive_for_steer(cx, store, id)?;
    if view.launched {
        return Ok(());
    }
    gone.finish(cx, store, id)?;
    Err(match view.reason {
        Some(failure) => CommandError::new(
            ErrorKind::Failed,
            format!(
                "the agent of Box {name} could not be relaunched: {}",
                failure.message
            ),
        ),
        None => gone.error,
    })
}

/// Ends the launch and Session of an agent that ended again soon after its relaunch, and the error.
///
/// # Errors
/// A store failure.
pub(super) fn ended_again(
    cx: &Cx,
    store: &Store,
    (id, name): (BoxId, &str),
    gone: &Gone,
) -> Result<CommandError, CommandError> {
    gone.finish(cx, store, id)?;
    let how = gone.code.map_or_else(
        || "lost its pane".to_owned(),
        |c| format!("exited with code {c}"),
    );
    Ok(CommandError::new(
        ErrorKind::Failed,
        format!(
            "the agent of Box {name} {how} again within {}s of its relaunch, so moor did not relaunch it and its session ended; start it again with `moor start {name} --resume`",
            RELAUNCH_GUARD.as_secs()
        ),
    )
    .with_detail(ErrorDetail::RelaunchCapped))
}

/// Whether the agent of `id` was relaunched less than `RELAUNCH_GUARD` ago.
fn relaunched_recently(cx: &Cx, store: &Store, id: BoxId) -> Result<bool, CommandError> {
    let now = cx.clock().now().as_millis();
    let window = i64::try_from(RELAUNCH_GUARD.as_millis()).unwrap_or(i64::MAX);
    Ok(store
        .events(id, RECENT_EVENTS)?
        .iter()
        .rev()
        .find(|e| e.kind == "relaunched")
        .is_some_and(|e| now.saturating_sub(e.ts.as_millis()) < window))
}
