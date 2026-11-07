//! Scripting a launch and a fate on an engine already in use.

use moor_core::seam::engine::{AgentFate, EngineError, LaunchReq, SessionRef};
use moor_core::seam::surface::PaneRef;

use super::FakeEngine;

impl FakeEngine {
    /// The next launch returns `r`; one that fails at step `cut` dies with its process instead.
    pub(crate) fn push_launch(&self, r: Result<SessionRef, EngineError>) {
        self.st().launch_results.push_back(r);
    }

    /// The next `agent_fate` returns `r`.
    pub(crate) fn push_fate(&self, r: Result<AgentFate, EngineError>) {
        self.st().fates.push_back(r);
    }

    /// Records the launch of `req` and answers as scripted, by default with pane `p-1`.
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
                pane: Some(PaneRef::new("p-1")),
                holder: st.holder,
            })
        })
    }
}
