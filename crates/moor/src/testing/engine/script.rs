//! Scripting a launch on an engine already in use.

use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};

use super::FakeEngine;

impl FakeEngine {
    /// The next launch returns `r`; one that fails at step `cut` dies with its process instead.
    pub(crate) fn push_launch(&self, r: Result<SessionRef, EngineError>) {
        self.st().launch_results.push_back(r);
    }

    /// Records the launch of `req` and answers as scripted, by default with a session of no id.
    pub(super) fn scripted_launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        let mut st = self.st();
        st.launches.push(req.clone());
        let next = st.launch_results.pop_front();
        if matches!(&next, Some(Err(EngineError::Failed { step: "cut", .. }))) {
            drop(st);
            std::panic::resume_unwind(Box::new("process died mid-launch"));
        }
        next.unwrap_or_else(|| {
            Ok(SessionRef {
                id: String::new(),
                handle: req.handle.clone(),
                workdir: req.workdir.clone(),
            })
        })
    }
}
