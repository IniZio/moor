//! A text steer waits in its Box's inbox until the agent can take it, and the Box's owner sends
//! the queue oldest first, stopping at the first row the agent cannot take now.

use std::time::Duration;

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::decide::inbox::{cause, next, AgentFact, Facts, Next};
use moor_core::domain::hands::HandsState;
use moor_core::domain::inbox::{Delivery as Row, InboxId, InboxKind, QueueCause, MAX_SENDS};
use moor_core::seam::engine::{EngineError, Steer};
use moor_core::seam::proc::Deadline;
use moor_core::view::steer::{Delivery, SteerValue};
use moor_core::{BoxId, LaunchId, SteerReq, Warning};

use super::{attempt, open_turn, record, reload, status_before, turn_before, Attempt, Gone, Mark};
use super::{bring_back, relaunch, Revived, Slot, STEER_DEADLINE};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::{self, session_ref, PaneLiveness, Reading};
use crate::ops::common::wake::wake_and_revive;
use crate::ops::start::{capped, revivable};
use crate::store::{BoxRow, InboxRow, Store};

/// Longest a steer waits for its Box's turn before it leaves its row queued.
const TURN_WAIT: Duration = Duration::from_secs(10);

/// Queues text `req` for Box `found` and sends it now when the Box is awake and its agent takes
/// it; otherwise says why it waits.
pub(super) fn steer(
    cx: &Cx,
    store: &Store,
    req: &SteerReq,
    found: &BoxRow,
) -> Result<SteerValue, CommandError> {
    let (id, name) = (found.id, found.name.as_str());
    let text = req.text.clone().unwrap_or_default();
    let inbox = store.enqueue(id, InboxKind::Steer, &text, InboxKind::Steer.as_str())?;
    let queued = |why| SteerValue {
        box_name: name.to_owned(),
        sent: text.clone(),
        answered_menu: false,
        steer: None,
        delivery: Delivery::Queued(why),
        inbox: inbox.get(),
    };
    let Ok(_turn) = cx.turn_within(id, Cmd::Steer, TURN_WAIT) else {
        return Ok(queued(cause(&unread(&reload(store, (id, name))?))));
    };
    let row = reload(store, (id, name))?;
    if !awake(&row) {
        if store.live_session(id)?.is_none() && (row.asleep() || !revivable(&row)) {
            store.withdraw(id, inbox)?;
            return Err(super::no_agent(name));
        }
        return Ok(queued(cause(&unread(&row))));
    }
    if store.unsent(id)?.first().map(|r| r.id) != Some(inbox) {
        let sent = deliver(cx, store, id)?;
        return Ok(sent.iter().find(|(r, _)| *r == inbox).map_or_else(
            || queued(QueueCause::Busy),
            |(_, steer)| SteerValue {
                steer: Some(*steer),
                delivery: Delivery::Delivered,
                ..queued(QueueCause::Busy)
            },
        ));
    }
    let mut slot = Slot::made(InboxKind::Steer, inbox, &text);
    match send_now(cx, store, req, (id, name), &mut slot) {
        Ok(Some(v)) => Ok(v),
        Ok(None) if capped(cx, store, id)? => Ok(queued(QueueCause::RelaunchCapped)),
        Ok(None) => Ok(queued(QueueCause::Relaunching)),
        Err(e) => {
            slot.withdraw(store, id)?;
            Err(e)
        }
    }
}

/// Sends `req` from `slot` to the agent of `target`, attaching anew to one that still runs at
/// most twice; none when the agent ended and the inbox pass relaunches it.
fn send_now(
    cx: &Cx,
    store: &Store,
    req: &SteerReq,
    target: (BoxId, &str),
    slot: &mut Slot,
) -> Result<Option<SteerValue>, CommandError> {
    let mut reattached = 0;
    loop {
        let mut gone = match attempt(cx, store, req, target, slot)? {
            Attempt::Sent(v) => return Ok(Some(v)),
            Attempt::Gone(gone) => gone,
        };
        match relaunch::recover(cx, store, target, &mut gone)? {
            Some(Revived::Relaunched) => {}
            Some(Revived::Running) if reattached < 2 => reattached += 1,
            Some(Revived::Running) => return Err(relaunch::lost_again(target.1)),
            None if revivable(&reload(store, target)?) => return Ok(None),
            None => {
                gone.finish(cx, store, target.0)?;
                return Err(gone.error);
            }
        }
    }
}

/// Sends Box `id`'s queued rows to its agent, oldest first, while it reads Idle or Working, and
/// returns each row sent with its `steered` event. A row out of sends, or one moor cannot send,
/// is reported and deleted, so it never holds the queue. The caller holds the Box's turn.
///
/// # Errors
/// A store failure, or a failed read of the Box.
pub(crate) fn deliver(
    cx: &Cx,
    store: &Store,
    id: BoxId,
) -> Result<Vec<(InboxId, i64)>, CommandError> {
    let mut sent = Vec::new();
    for item in store.unsent(id)? {
        let Some(row) = store.box_row(id)? else {
            break;
        };
        let live = store.live_session(id)?;
        if spent(&item, live.as_ref().map(|l| LaunchId::new(l.id))) {
            store.unconfirmed(id, item.id)?;
            continue;
        }
        let Some(launch) = live else { break };
        let reading = activity::read(cx, store, &row, cx.clock().now(), activity::PROBE_DEADLINE)?;
        let Some(sref) = session_ref(&row, &launch).filter(|_| ready(&reading)) else {
            break;
        };
        store.mark_sending(id, item.id, LaunchId::new(launch.id))?;
        let steer = Steer::Text(item.text.clone());
        match cx.engine().steer(&sref, &steer, Deadline(STEER_DEADLINE)) {
            Ok(outcome) => {
                let mark = Mark {
                    turn_before: turn_before(reading.term.as_ref(), &steer),
                    status_before: status_before(reading.term.as_ref()),
                    submit_retried: outcome.submit_retried,
                };
                sent.push((
                    item.id,
                    record(store, id, &launch, &item.text, mark, item.id)?,
                ));
                open_turn(store, &row)?;
            }
            Err(EngineError::AgentGone) => {
                store.refuse(id, item.id)?;
                break;
            }
            Err(_) => {
                store.rollback(id, item.id)?;
                break;
            }
        }
    }
    Ok(sent)
}

/// Moves each Box with queued rows towards its agent taking them: wakes an asleep Box, brings
/// one that lost its machine back on new hands, starts a dead agent again, and delivers to a
/// ready one. A Box another op holds is left for a later pass, until `draining`.
///
/// # Errors
/// A store failure listing the Boxes.
pub(crate) fn pass(
    cx: &Cx,
    store: &Store,
    draining: &dyn Fn() -> bool,
) -> Result<(), CommandError> {
    for id in store.boxes_with_unsent()? {
        if draining() {
            break;
        }
        let Some(row) = store.box_row(id)? else {
            continue;
        };
        if !row.lifecycle.is_live() {
            continue;
        }
        cx.for_box(row.user, || {
            if let Err(e) = move_on(cx, store, &row) {
                cx.warn(Warning {
                    code: "inbox_failed".to_owned(),
                    message: format!("could not deliver to Box {}: {}", id.get(), e.error),
                });
            }
        });
    }
    Ok(())
}

/// One step of [`pass`] for Box `row`.
fn move_on(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let id = row.id;
    let mut facts = unread(row);
    let reading = if awake(row) {
        let reading = activity::read(cx, store, row, cx.clock().now(), activity::PROBE_DEADLINE)?;
        facts.agent = agent(&reading);
        facts.capped = facts.agent == AgentFact::Dead && capped(cx, store, id)?;
        Some(reading)
    } else {
        None
    };
    match next(&facts) {
        Next::Hold => Ok(()),
        Next::BringBack if cx.owners().busy(id) => Ok(()),
        Next::BringBack => bring_back(cx, row),
        Next::Wake => {
            let Some(_turn) = cx.try_turn(id, Cmd::Wake) else {
                return Ok(());
            };
            wake_and_revive(cx, store, &reload(store, (id, row.name.as_str()))?)?;
            deliver(cx, store, id).map(drop)
        }
        Next::Revive | Next::Deliver => {
            let Ok(_turn) = cx.turn_within(id, Cmd::Steer, Duration::ZERO) else {
                return Ok(());
            };
            if facts.agent == AgentFact::Dead {
                let code = reading.and_then(|r| match r.activity {
                    Some(Activity {
                        state: AgentState::Exited { code },
                        ..
                    }) => code,
                    _ => None,
                });
                let gone = Gone::new(row.name.as_str(), store.live_session(id)?, code);
                if let Err(e) = relaunch::once(cx, store, (id, row.name.as_str()), gone) {
                    if !revivable(&reload(store, (id, row.name.as_str()))?) {
                        for item in store.unsent(id)? {
                            store.unconfirmed(id, item.id)?;
                        }
                    }
                    return Err(e);
                }
            }
            deliver(cx, store, id).map(drop)
        }
    }
}

/// Whether row `item` may not be sent again: out of sends, or in doubt on a launch that ended.
fn spent(item: &InboxRow, live: Option<LaunchId>) -> bool {
    let kept = matches!(
        item.kind,
        InboxKind::Steer | InboxKind::Prompt | InboxKind::Note
    );
    !kept
        || match item.delivery {
            Row::Queued { attempts } => attempts >= MAX_SENDS,
            Row::InDoubt { launch, attempts } => attempts >= MAX_SENDS || Some(launch) != live,
            Row::Sent { .. } => false,
        }
}

/// Whether `reading` shows a live agent that takes a message now.
const fn ready(reading: &Reading) -> bool {
    !reading.read_failed
        && matches!(
            reading.activity,
            Some(Activity {
                state: AgentState::Idle | AgentState::Working,
                source: Source::Status,
            })
        )
}

/// What `reading` shows of the agent of an awake Box.
const fn agent(reading: &Reading) -> AgentFact {
    let ended = matches!(reading.pane, PaneLiveness::Exited | PaneLiveness::Gone);
    if ended && reading.launch_failure.is_none() {
        AgentFact::Dead
    } else if ready(reading) {
        AgentFact::Ready
    } else {
        AgentFact::NotReady
    }
}

/// Where Box `row` stands, its agent not read.
fn unread(row: &BoxRow) -> Facts {
    Facts {
        lifecycle: row.lifecycle,
        hands: row.hands.as_ref().map(|h| h.state),
        agent: AgentFact::NotReady,
        capped: false,
    }
}

/// Whether Box `row` and its machine are up.
fn awake(row: &BoxRow) -> bool {
    !row.asleep()
        && row.lifecycle == moor_core::domain::box_state::BoxLifecycle::Ready
        && row
            .hands
            .as_ref()
            .is_some_and(|h| h.state == HandsState::Ready)
}
