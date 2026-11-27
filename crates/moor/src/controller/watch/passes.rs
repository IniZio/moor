//! Which stored events a watch stream sends.

use std::collections::HashMap;

use moor_core::controller::ServerFrame;
use moor_core::{BoxId, SessionId};

use super::Ask;
use crate::Moor;

/// Which stored events one stream sends: those its filter matches, of Boxes its viewer may
/// see. An event with no Box is the operator's alone.
pub(super) struct Passes<'a> {
    /// The engine seeing as the stream's viewer.
    pub(super) moor: &'a Moor,
    /// What the client asked for.
    pub(super) ask: &'a Ask,
    /// Whether the viewer may see each Box met so far; a Box's user never changes.
    pub(super) seen: HashMap<BoxId, bool>,
    /// The parent of each Session met so far; a Session's parent never changes.
    pub(super) parents: HashMap<SessionId, Option<SessionId>>,
}

impl<'a> Passes<'a> {
    /// The passes of one stream, which have met no Box yet.
    pub(super) fn new(moor: &'a Moor, ask: &'a Ask) -> Self {
        Self {
            moor,
            ask,
            seen: HashMap::new(),
            parents: HashMap::new(),
        }
    }

    /// Whether `frame` goes to the client.
    pub(super) fn check(&mut self, frame: &ServerFrame) -> Result<bool, String> {
        let ServerFrame::Event {
            r#box,
            box_id,
            session,
            kind,
            ..
        } = frame
        else {
            return Ok(true);
        };
        if !self.ask.filter.matches(r#box.as_deref(), kind) {
            return Ok(false);
        }
        if let Some(parent) = self.ask.filter.parent {
            if self.parent_of(*session)? != Some(parent) {
                return Ok(false);
            }
        }
        let Some(id) = *box_id else {
            return Ok(self.ask.viewer.may_act_on(None));
        };
        if let Some(&visible) = self.seen.get(&id) {
            return Ok(visible);
        }
        let visible = self.moor.may_see(id)?;
        self.seen.insert(id, visible);
        Ok(visible)
    }

    /// The parent of `session`; none for an event with no Session.
    fn parent_of(&mut self, session: Option<SessionId>) -> Result<Option<SessionId>, String> {
        let Some(id) = session else { return Ok(None) };
        if let Some(&parent) = self.parents.get(&id) {
            return Ok(parent);
        }
        let parent = self.moor.session_parent(id)?;
        self.parents.insert(id, parent);
        Ok(parent)
    }
}
