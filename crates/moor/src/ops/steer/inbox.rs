//! The inbox row a text steer waits in from just before its send until its turn ends.

use moor_core::domain::inbox::{InboxId, InboxKind};
use moor_core::seam::engine::{SessionRef, Steer, SteerOutcome};
use moor_core::{BoxId, LaunchId};

use super::{is_exit, send};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// The inbox row a text waits in, made at its first send.
pub(super) struct Slot {
    /// Who the text is from.
    kind: InboxKind,
    /// The row, once made.
    row: Option<InboxId>,
}

impl Slot {
    /// No row yet for a text of `kind`.
    pub(super) const fn new(kind: InboxKind) -> Self {
        Self { kind, row: None }
    }

    /// Marks the row of `text` as being sent to `launch`, making it first if need be.
    fn mark(
        &mut self,
        store: &Store,
        id: BoxId,
        text: &str,
        launch: &SessionRow,
    ) -> Result<InboxId, CommandError> {
        let row = match self.row {
            Some(row) => row,
            None => *self
                .row
                .insert(store.enqueue(id, self.kind, text, self.kind.as_str())?),
        };
        store.mark_sending(id, row, LaunchId::new(launch.id))?;
        Ok(row)
    }

    /// Sends `steer` to the agent of `launch`, a text from the inbox; what [`send`] says, and the
    /// row it went from. A text a gone agent took no send for is dropped, and the next try makes
    /// a fresh row, so a steer is sent as often as before it waited in the inbox.
    pub(super) fn send(
        &mut self,
        cx: &Cx,
        store: &Store,
        target: (BoxId, &str),
        (sref, launch): (&SessionRef, &SessionRow),
        steer: &Steer,
    ) -> Result<(Option<SteerOutcome>, Option<InboxId>), CommandError> {
        let inbox = match steer {
            Steer::Text(t) if !is_exit(steer) => Some(self.mark(store, target.0, t, launch)?),
            _ => None,
        };
        let outcome = send(cx, sref, steer)?;
        if let (None, Some(row)) = (&outcome, inbox) {
            store.withdraw(target.0, row)?;
            self.row = None;
        }
        Ok((outcome, inbox))
    }

    /// Deletes the row unless it was sent: a text refused, or sent as a menu choice, leaves
    /// nothing queued.
    pub(super) fn withdraw(&self, store: &Store, id: BoxId) -> Result<(), CommandError> {
        if let Some(row) = self.row {
            store.withdraw(id, row)?;
        }
        Ok(())
    }
}
