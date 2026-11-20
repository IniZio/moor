//! The pass that closes Boxes nobody needs any more, so their disks do not pile up.
//!
//! A workflow run's Box closes a while after its run ends, counted from the last steer or terminal
//! someone opened since. A one-off Box closes when its pull request merged or closed, or when it sat
//! idle for long enough. A Box holding work that exists only in it is never closed: the pass says
//! so on the Box and leaves it to a person. The close never wakes an asleep Box; it answers from
//! the check recorded when the Box went to sleep.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::AutoClose;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::close_reason::{
    CLOSE_REASON_IDLE, CLOSE_REASON_PR_CLOSED, CLOSE_REASON_PR_MERGED, CLOSE_REASON_RUN_ENDED,
};
use moor_core::domain::session_state::SessionState;
use moor_core::response::{CloseRefusal, ErrorDetail};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::PullState;
use moor_core::{BoxId, BoxReq, BoxTarget, CloseReq};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::{close, pulls};
use crate::store::{BoxRow, RunOfBox, Store, StoreError};

/// Events that show someone steered or opened the terminal of a Box.
const HANDS_ON: [&str; 2] = ["steered", "terminal_opened"];

/// Events that show a one-off Box was used: a person's hands, or its agent's turns.
const IN_USE: [&str; 6] = [
    "started",
    "steered",
    "terminal_opened",
    "idled",
    "turn_done",
    "needs_input",
];

/// How many passes a Box waits before the pass looks again at a pull request or a held close.
const RECHECK_PASSES: u32 = 5;

/// When the pass last read each Box's pull requests, so the forge is asked far less often than
/// the pass runs. One per controller.
#[derive(Default)]
pub(crate) struct Looked(Mutex<HashMap<BoxId, Ts>>);

impl Looked {
    /// Whether Box `id` was read at or after `since`; if not, notes it as read at `now`.
    fn due(&self, id: BoxId, (since, now): (Ts, Ts)) -> bool {
        let mut seen = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.get(&id).is_some_and(|at| *at >= since) {
            return false;
        }
        seen.insert(id, now);
        true
    }
}

/// Closes each open Box that is due, as the config says. Returns what to log: each close and each
/// failure. A Box another op holds, one with something wanted of it and one at work are left for
/// a later pass.
///
/// # Errors
/// A store failure listing the Boxes.
pub(crate) fn pass(
    cx: &Cx,
    store: &Store,
    draining: &dyn Fn() -> bool,
    (cfg, looked): (&AutoClose, &Looked),
) -> Result<Vec<String>, CommandError> {
    let mut out = Vec::new();
    for row in store.open_boxes()? {
        if draining() {
            break;
        }
        let open = matches!(row.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep);
        if !open || row.wanted.is_some() || cx.owners().busy(row.id) {
            continue;
        }
        match close_if_due(cx, store, &row, (cfg, looked)) {
            Ok(Some(line)) => out.push(line),
            Ok(None) => {}
            Err(e) => out.push(format!("auto-close of {}: {}", row.name.as_str(), e.error)),
        }
    }
    Ok(out)
}

/// What made a close due.
#[derive(Clone, Copy)]
enum Due {
    /// The clock: a run's Box or an idle one-off Box, judged again under the Box's turn.
    Clock(&'static str),
    /// The Box's pull request ended.
    Pull(&'static str),
}

impl Due {
    /// The reason on the `closed` event.
    const fn reason(self) -> &'static str {
        match self {
            Self::Clock(r) | Self::Pull(r) => r,
        }
    }
}

/// Closes `row` when it is due and nothing holds it; the line to log when it closed.
fn close_if_due(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (cfg, looked): (&AutoClose, &Looked),
) -> Result<Option<String>, CommandError> {
    let now = cx.clock().now();
    let wait = cfg.every * RECHECK_PASSES;
    let Some(due) = due(cx, store, row, (cfg, looked), now)? else {
        return Ok(None);
    };
    if held_lately(store, row.id, now, wait)? {
        return Ok(None);
    }
    let kept = Cell::new(false);
    let keep = |store: &Store, r: &BoxRow| -> Option<String> {
        let why = still_wanted(cx, store, r, cfg, due);
        kept.set(why.is_some());
        why
    };
    let req = CloseReq {
        target: BoxTarget::from(row.id),
        force: false,
    };
    let closed = cx.for_box(row.user, || close::run_auto(cx, &req, due.reason(), &keep));
    match closed {
        Ok(_) => Ok(Some(format!(
            "closed {}: {}",
            row.name.as_str(),
            due.reason()
        ))),
        Err(_) if kept.get() => Ok(None),
        Err(e) => match e.detail.as_deref() {
            Some(ErrorDetail::CloseRefused(r)) => {
                hold(cx, store, row, due, held_for(r))?;
                Ok(None)
            }
            _ => Err(e),
        },
    }
}

/// Why Box `row` should still stay open under its turn, or none when it may close.
fn still_wanted(cx: &Cx, store: &Store, row: &BoxRow, cfg: &AutoClose, due: Due) -> Option<String> {
    if matches!(row.session, SessionState::Working | SessionState::Starting) {
        return Some("its agent is working".to_owned());
    }
    let Due::Clock(_) = due else { return None };
    match due_by_clock(store, row, cfg, cx.clock().now()) {
        Ok(Some(_)) => None,
        Ok(None) => Some("it was used again".to_owned()),
        Err(e) => Some(format!("could not read its activity: {e}")),
    }
}

/// What made closing `row` due now, when anything did.
fn due(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (cfg, looked): (&AutoClose, &Looked),
    now: Ts,
) -> Result<Option<Due>, CommandError> {
    if let Some(reason) = due_by_clock(store, row, cfg, now)? {
        return Ok(Some(Due::Clock(reason)));
    }
    if store.run_of_box(row.id)? != RunOfBox::NoRun {
        return Ok(None);
    }
    let since = Ts::from_millis(now.as_millis() - millis(cfg.every * RECHECK_PASSES));
    if !looked.due(row.id, (since, now)) {
        return Ok(None);
    }
    Ok(pull_ended(cx, row).map(Due::Pull))
}

/// A duration in milliseconds.
fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// The reason Box `row` is due by the clock at `now`, when it is: its run ended long enough ago
/// with nobody's hands on it since, or it is a one-off Box left idle for long enough.
fn due_by_clock(
    store: &Store,
    row: &BoxRow,
    cfg: &AutoClose,
    now: Ts,
) -> Result<Option<&'static str>, StoreError> {
    let (after, from, reason) = match store.run_of_box(row.id)? {
        RunOfBox::Open => return Ok(None),
        RunOfBox::Ended(ended) => {
            let Some(after) = cfg.run_after else {
                return Ok(None);
            };
            let touched = store.last_event_at(row.id, &HANDS_ON)?;
            (
                after,
                touched.map_or(ended, |t| t.max(ended)),
                CLOSE_REASON_RUN_ENDED,
            )
        }
        RunOfBox::NoRun => {
            let Some(after) = cfg.idle_after else {
                return Ok(None);
            };
            let Some(last) = store.last_event_at(row.id, &IN_USE)? else {
                return Ok(None);
            };
            (after, last, CLOSE_REASON_IDLE)
        }
    };
    Ok((now >= from.saturating_add(after)).then_some(reason))
}

/// The reason Box `row` is due when every pull request from its branch merged or closed, and at
/// least one exists. Any repo that cannot be read, or has an open pull request, keeps it open.
fn pull_ended(cx: &Cx, row: &BoxRow) -> Option<&'static str> {
    row.branch.as_ref()?;
    row.user?;
    let req = BoxReq {
        box_name: row.name.clone(),
    };
    let read = cx.for_box(row.user, || pulls::run(cx, &req)).ok()?;
    if read.repos.iter().any(|r| r.unread.is_some()) {
        return None;
    }
    let states: Vec<_> = read
        .repos
        .iter()
        .filter_map(|r| r.pull.as_ref())
        .map(|p| p.state)
        .collect();
    if states.is_empty() || states.iter().any(|s| s.is_open()) {
        return None;
    }
    if states.iter().all(|s| *s == PullState::Merged) {
        Some(CLOSE_REASON_PR_MERGED)
    } else {
        Some(CLOSE_REASON_PR_CLOSED)
    }
}

/// What a refusal of the gate says holds the Box, in words.
fn held_for(r: &CloseRefusal) -> String {
    match r {
        CloseRefusal::Unsaved { ahead, dirty, .. } => {
            format!("it holds {ahead} unpushed commit(s) and {dirty} uncommitted file(s)")
        }
        CloseRefusal::Unchecked { reason } | CloseRefusal::WaitingForRoom { reason } => {
            reason.clone()
        }
        CloseRefusal::AlreadyClosed => "it is already closed".to_owned(),
    }
}

/// Whether a close of Box `id` was held back less than `wait` ago, so it is not tried again yet.
fn held_lately(store: &Store, id: BoxId, now: Ts, wait: Duration) -> Result<bool, StoreError> {
    let newest = store.newest_events_of(id, "auto_close_held", 1)?.pop();
    Ok(newest.is_some_and(|e| now < e.ts.saturating_add(wait)))
}

/// Records on Box `row` that its close was due for `due` but held back for `why`, unless its
/// newest such event already says so.
fn hold(cx: &Cx, store: &Store, row: &BoxRow, due: Due, why: String) -> Result<(), CommandError> {
    let said = store.newest_events_of(row.id, "auto_close_held", 1)?.pop();
    let same = said.is_some_and(|e| {
        e.payload.get("why").and_then(|w| w.as_str()) == Some(why.as_str())
            && e.payload.get("due").and_then(|w| w.as_str()) == Some(due.reason())
    });
    if same {
        return Ok(());
    }
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(());
    };
    let event = ExtraEvent::AutoCloseHeld {
        due: due.reason().to_owned(),
        why,
    };
    store.apply(Decision::new(row.id).event(event))?;
    Ok(())
}
