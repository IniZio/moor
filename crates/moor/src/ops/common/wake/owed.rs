//! What a wake still owes the agent of a Box that stopped mid-turn, given from a status sweep.

use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::BoxId;

use super::tell_after_suspend;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::{PaneLiveness, Reading};
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::link::Fate;
use crate::ops::start::{revive_after_wake, AfterWake};
use crate::store::{BoxRow, Store};

/// What a status sweep's read found of the agent of an awake Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// The Box answered and its agent runs, in its pane or a new one.
    Running,
    /// The Box said its agent ended.
    Ended,
    /// The Box gave no answer about its agent.
    Silent,
}

impl Seen {
    /// What `reading` and the `fate` its settle decided show of the agent.
    pub(crate) fn of(reading: &Reading, fate: Option<Fate>) -> Self {
        match fate {
            Some(Fate::Reattached) => Self::Running,
            Some(Fate::Ended(_)) => Self::Ended,
            None if reading.pane == PaneLiveness::Live && !reading.read_failed => Self::Running,
            Some(Fate::Unreachable) | None => Self::Silent,
        }
    }
}

/// Gives the agent of an awake Box that stopped mid-turn what its wake could not, from what a
/// sweep's read found, and asks the Box nothing more: a silent Box waits for a later sweep. A
/// running agent is told, even one that went quiet since. An agent the Box says ended is started
/// again once; when that launch fails, or no session is left, the mark is dropped. A Box another
/// op holds is left for a later sweep.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(crate) fn resume_owed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    seen: Seen,
) -> Result<(), CommandError> {
    if store.resume_first(id)?.is_none() {
        return Ok(());
    }
    let Ok(_held) = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "resume", Duration::ZERO)
    else {
        return Ok(());
    };
    match store.box_row(id)? {
        Some(row) if row.lifecycle == BoxLifecycle::Ready => give(cx, store, &row, seen),
        _ => Ok(()),
    }
}

/// Gives the agent of Ready `row`, whose lock the caller holds, what `seen` shows it is owed.
fn give(cx: &Cx, store: &Store, row: &BoxRow, seen: Seen) -> Result<(), CommandError> {
    let id = row.id;
    let after = match (seen, store.live_session(id)?.is_some()) {
        (Seen::Ended, false) => revive_after_wake(cx, store, id)?,
        (Seen::Running, true) => AfterWake::Intact,
        (_, true) => return Ok(()),
        (_, false) => AfterWake::Unknown,
    };
    if after == AfterWake::Unknown {
        store.set_resume_first(id, None)?;
        return Ok(());
    }
    tell_after_suspend(cx, store, row, after)?;
    crate::ops::common::held_prompt::give_after_wake(cx, store, row, after)
}
