//! The inbox row a steer waits in from just before its send until its turn ends.

use moor_core::domain::inbox::{InboxId, InboxKind};
use moor_core::seam::engine::{SessionRef, Steer, SteerOutcome};
use moor_core::{BoxId, LaunchId};

use super::{is_exit, send};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// The inbox row a steer is sent from, made at its first send.
pub(super) struct Slot {
    /// Who a text is from.
    kind: InboxKind,
    /// The row, once made, with what it holds.
    row: Option<(InboxId, InboxKind, String)>,
}

impl Slot {
    /// No row yet for a steer whose text is of `kind`.
    pub(super) const fn new(kind: InboxKind) -> Self {
        Self { kind, row: None }
    }

    /// Row `row`, already queued with `text` of `kind`.
    pub(super) fn made(kind: InboxKind, row: InboxId, text: &str) -> Self {
        Self {
            kind,
            row: Some((row, kind, text.to_owned())),
        }
    }

    /// Marks the row of `text` of `kind` as being sent to `launch`. A retry that sends something
    /// else, such as a text the relaunched agent's menu turned into a choice, gets a row of its own.
    fn mark(
        &mut self,
        store: &Store,
        id: BoxId,
        (kind, text): (InboxKind, &str),
        launch: &SessionRow,
    ) -> Result<InboxId, CommandError> {
        let row = match &self.row {
            Some((row, k, t)) if *k == kind && t == text => *row,
            _ => {
                self.withdraw(store, id)?;
                let row = store.enqueue(id, kind, text, self.kind.as_str())?;
                self.row = Some((row, kind, text.to_owned()));
                row
            }
        };
        store.mark_sending(id, row, LaunchId::new(launch.id))?;
        Ok(row)
    }

    /// Sends `steer`, recorded as `sent`, to the agent of `launch` from its row; what [`send`]
    /// says, and the row. A gone agent's terminal was missing or had ended, so it typed no key,
    /// and the refused send is undone without counting; an `/exit` it refused still left the
    /// agent exited, as asked, so its row stays to be confirmed.
    pub(super) fn send(
        &mut self,
        cx: &Cx,
        store: &Store,
        target: (BoxId, &str),
        (sref, launch): (&SessionRef, &SessionRow),
        (steer, sent): (&Steer, &str),
    ) -> Result<(Option<SteerOutcome>, InboxId), CommandError> {
        let kind = match steer {
            Steer::Text(_) => self.kind,
            Steer::Menu(_) => InboxKind::Choice,
            Steer::Interrupt => InboxKind::Interrupt,
        };
        let row = self.mark(store, target.0, (kind, sent), launch)?;
        let outcome = send(cx, sref, steer)?;
        if outcome.is_none() && !is_exit(steer) {
            store.refuse(target.0, row)?;
        }
        Ok((outcome, row))
    }

    /// Deletes the row unless it was sent: a steer refused leaves nothing queued.
    pub(super) fn withdraw(&self, store: &Store, id: BoxId) -> Result<(), CommandError> {
        if let Some((row, ..)) = &self.row {
            store.withdraw(id, *row)?;
        }
        Ok(())
    }
}
