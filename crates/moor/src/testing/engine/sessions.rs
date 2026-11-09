//! Which Box holds which of the agent's stored sessions, and what a resume found.

use std::collections::HashSet;

use moor_core::seam::engine::{EngineError, LaunchReq};

use super::FakeEngine;

/// The stored sessions each Box holds, as (handle, session id).
pub(super) type Held = HashSet<(String, String)>;

impl FakeEngine {
    /// Only the Box `handle` holds session `id`, and other Boxes hold what is restored to them.
    /// Unscripted, every Box holds every session.
    pub(crate) fn holding(self, handle: &str, id: &str) -> Self {
        self.st()
            .held
            .get_or_insert_with(Held::new)
            .insert((handle.to_owned(), id.to_owned()));
        self
    }

    /// The next restore fails once with `e`.
    pub(crate) fn fail_restore(self, e: EngineError) -> Self {
        self.st().fail_restore = Some(e);
        self
    }

    /// Each restore as (handle, session id, archive bytes), in order.
    pub(crate) fn restores(&self) -> Vec<(String, String, Vec<u8>)> {
        self.st().restores.clone()
    }

    /// Each resuming launch as (handle, session id, whether that Box held the session).
    pub(crate) fn resumes(&self) -> Vec<(String, String, bool)> {
        self.st().resumes.clone()
    }

    pub(super) fn holds(&self, req: &LaunchReq, id: &str) -> bool {
        self.st()
            .held
            .as_ref()
            .is_none_or(|h| h.contains(&(req.handle.as_str().to_owned(), id.to_owned())))
    }

    pub(super) fn note_resume(&self, req: &LaunchReq) {
        if let Some(id) = &req.resume {
            let found = self.holds(req, id);
            let handle = req.handle.as_str().to_owned();
            self.st().resumes.push((handle, id.clone(), found));
        }
    }

    pub(super) fn restore(
        &self,
        req: &LaunchReq,
        id: &str,
        archive: &[u8],
    ) -> Result<(), EngineError> {
        let mut st = self.st();
        let handle = req.handle.as_str().to_owned();
        st.restores
            .push((handle.clone(), id.to_owned(), archive.to_vec()));
        if let Some(e) = st.fail_restore.take() {
            return Err(e);
        }
        st.held
            .get_or_insert_with(Held::new)
            .insert((handle, id.to_owned()));
        Ok(())
    }
}
