//! The inbox row a steer waits in from just before its send until its turn ends.

use moor_core::domain::event::{Origin, MOOR_SENDER};
use moor_core::domain::inbox::{Delivery, InboxId, InboxKind};
use moor_core::seam::engine::{SessionRef, Steer, SteerOutcome};
use moor_core::{BoxId, SteerReq};

use super::{attempt, is_exit, send, Attempt};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::failpoint;
use crate::store::{BoxRow, Launch, Store};

/// The inbox row a steer is sent from, made at its first send.
pub(super) struct Slot {
    /// Who a text is from.
    kind: InboxKind,
    /// Who wrote the text, which the row's sender keeps for the inbox pass.
    origin: Origin,
    /// The row, once made, with what it holds.
    row: Option<(InboxId, InboxKind, String)>,
}

impl Slot {
    /// No row yet for a steer whose text is of `kind`.
    pub(super) const fn new(kind: InboxKind) -> Self {
        Self {
            kind,
            origin: Origin::Person,
            row: None,
        }
    }

    /// The same slot for a text moor wrote.
    pub(super) const fn by(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// Who wrote the text.
    pub(super) const fn origin(&self) -> Origin {
        self.origin
    }

    /// The sender an inbox row of `kind` carries when `origin` wrote it.
    pub(super) const fn sender(kind: InboxKind, origin: Origin) -> &'static str {
        match origin {
            Origin::Moor => MOOR_SENDER,
            Origin::Person => kind.as_str(),
        }
    }

    /// Row `row`, already queued with `text` of `kind`.
    pub(super) fn made(kind: InboxKind, row: InboxId, text: &str) -> Self {
        Self {
            kind,
            origin: Origin::Person,
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
        launch: &Launch,
    ) -> Result<InboxId, CommandError> {
        let row = match &self.row {
            Some((row, k, t)) if *k == kind && t == text => *row,
            _ => {
                self.withdraw(store, id)?;
                let row = store.enqueue(id, kind, text, Self::sender(self.kind, self.origin))?;
                self.row = Some((row, kind, text.to_owned()));
                row
            }
        };
        store.mark_sending(id, row, launch.id)?;
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
        (sref, launch): (&SessionRef, &Launch),
        (steer, sent): (&Steer, &str),
    ) -> Result<(Option<SteerOutcome>, InboxId), CommandError> {
        let kind = match steer {
            Steer::Text(_) => self.kind,
            Steer::Menu(_) => InboxKind::Choice,
            Steer::Interrupt => InboxKind::Interrupt,
        };
        let row = self.mark(store, target.0, (kind, sent), launch)?;
        failpoint::hit(failpoint::INBOX_BEFORE_SEND);
        let outcome = send(cx, sref, steer)?;
        if outcome.is_some() {
            failpoint::hit(failpoint::INBOX_AFTER_SEND);
        } else if !is_exit(steer) {
            store.refuse(target.0, row)?;
        }
        Ok((outcome, row))
    }

    /// The row, when a send of it began and was never confirmed, with whether it holds a text the
    /// inbox may send once more rather than a key for a screen gone by.
    pub(super) fn in_doubt(
        &self,
        store: &Store,
        id: BoxId,
    ) -> Result<Option<(InboxId, bool)>, CommandError> {
        let Some((row, kind, _)) = &self.row else {
            return Ok(None);
        };
        let doubt = store.unsent(id)?.into_iter().find(|r| r.id == *row);
        let text = matches!(kind, InboxKind::Steer | InboxKind::Prompt | InboxKind::Note);
        Ok(doubt
            .filter(|r| matches!(r.delivery, Delivery::InDoubt { .. }))
            .map(|r| (r.id, text)))
    }

    /// Ends a steer sent only now, such as a choice, interrupt or `/exit`: withdraws its row, and
    /// when the steer `failed` with the row in doubt, reports and deletes it, since sent later it
    /// would act on a screen nobody sees by then.
    pub(super) fn give_up(
        &self,
        store: &Store,
        id: BoxId,
        failed: bool,
    ) -> Result<(), CommandError> {
        self.withdraw(store, id)?;
        if let Some((row, _)) = self.in_doubt(store, id)?.filter(|_| failed) {
            store.unconfirmed(id, row)?;
        }
        Ok(())
    }

    /// Deletes the row unless a send of it began: a steer refused leaves nothing queued, and one
    /// whose send failed or went unconfirmed stays in doubt, to be sent at most once more.
    pub(super) fn withdraw(&self, store: &Store, id: BoxId) -> Result<(), CommandError> {
        if let Some((row, ..)) = &self.row {
            store.withdraw(id, *row)?;
        }
        Ok(())
    }
}

/// Sends `text` to the live agent of awake Box `row`, the way a steer does.
pub(crate) fn tell(cx: &Cx, store: &Store, row: &BoxRow, text: String) -> Result<(), CommandError> {
    let req = SteerReq {
        target: moor_core::request::BoxTarget::Id(row.id),
        text: Some(text),
        menu: None,
        interrupt: false,
        ask: None,
    };
    let mut slot = Slot::new(InboxKind::Note).by(Origin::Moor);
    let sent = attempt(cx, store, &req, (row.id, row.name.as_str()), &mut slot);
    slot.withdraw(store, row.id)?;
    match sent? {
        Attempt::Sent(_) => Ok(()),
        Attempt::Gone(gone) => Err(gone.error),
    }
}
