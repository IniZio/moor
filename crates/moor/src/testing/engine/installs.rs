//! What the fake Engine installs.

use moor_core::seam::agent_adapter::AgentInstall;

use super::FakeEngine;

impl FakeEngine {
    /// The Engine runs `script` for a profile's plugin installs.
    pub(crate) fn profile_install(self, script: &str) -> Self {
        self.st().profile_install = Some(script.to_owned());
        self
    }

    /// The Engine knows how to install its agent with `install`.
    pub(crate) fn install(self, install: AgentInstall) -> Self {
        self.st().install = Some(install);
        self
    }
}
