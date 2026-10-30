//! The Box agent kit, the one Engine adapter; agent-specific behaviour sits behind an `AgentAdapter`.

mod agent_env;
mod brief;
pub(crate) mod claude;
pub(crate) mod gh_release;
mod launch;

pub(crate) use launch::detach::relay_argv;
mod probe;
mod proxy_env;
mod steer;
mod transcript;

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::Probes;
use moor_core::name::{BoxName, BoxPath};
use moor_core::seam::agent_adapter::{AgentAdapter, AgentInstall, PromptMatch};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::{
    AgentFate, Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, Surface, WorkspaceRef};

use self::gh_release::GhRelease;

/// An agent running in a Box, read through the Driver and the Surface.
pub(crate) struct BoxAgent {
    /// Reads the hook file out of the Box.
    driver: Arc<dyn Driver>,
    /// Observes and opens the agent's pane.
    surface: Arc<dyn Surface>,
    /// Parses the agent's hook file and builds its launch.
    profile: Arc<dyn AgentAdapter>,
    /// Paces the submit key after typing.
    clock: Arc<dyn Clock>,
    /// Fetches the pinned gh for a Box that lacks it.
    release: Arc<GhRelease>,
}

impl BoxAgent {
    /// A kit over the given Driver, Surface, agent adapter, clock and gh source.
    pub(crate) fn new(
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        profile: Arc<dyn AgentAdapter>,
        clock: Arc<dyn Clock>,
        release: Arc<GhRelease>,
    ) -> Self {
        Self {
            driver,
            surface,
            profile,
            clock,
            release,
        }
    }
}

impl Engine for BoxAgent {
    fn relay_argv(&self, connect: &[String]) -> Option<Vec<String>> {
        Some(relay_argv(connect))
    }

    fn probe(&self, s: &SessionRef, deadline: Deadline) -> Result<Probes, EngineError> {
        probe::probe(
            self.driver.as_ref(),
            self.surface.as_ref(),
            self.profile.as_ref(),
            s,
            deadline,
        )
    }

    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch> {
        self.profile.first_run_prompt(tail)
    }

    fn hooks_quiet_during_first_run(&self) -> bool {
        self.profile.hooks_quiet_during_first_run()
    }

    fn agent_install(&self) -> Option<AgentInstall> {
        self.profile.install()
    }

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        launch::launch(
            self.driver.as_ref(),
            self.surface.as_ref(),
            self.profile.as_ref(),
            req,
            &self.release,
        )
    }

    fn launch_dirs(&self, req: &LaunchReq) -> Result<Option<(BoxPath, BoxPath)>, EngineError> {
        launch::dirs(self.driver.as_ref(), req).map(Some)
    }

    fn steer(
        &self,
        s: &SessionRef,
        steer: &Steer,
        deadline: Deadline,
    ) -> Result<SteerOutcome, EngineError> {
        let sent = steer::steer(
            self.surface.as_ref(),
            self.profile.as_ref(),
            self.clock.as_ref(),
            s,
            steer,
            deadline,
        )?;
        let _ = self.driver.touch(&s.handle);
        Ok(sent)
    }

    fn transcript(&self, s: &SessionRef, dest: &Path) -> Result<(), EngineError> {
        transcript::save(self.driver.as_ref(), self.profile.as_ref(), s, dest)
    }

    fn stop(&self, s: &SessionRef) -> Result<(), EngineError> {
        launch::detach::stop(self.driver.as_ref(), s)
    }

    fn agent_fate(&self, s: &SessionRef, deadline: Deadline) -> Result<AgentFate, EngineError> {
        launch::detach::fate(self.driver.as_ref(), s, deadline)
    }

    fn reattach(
        &self,
        s: &SessionRef,
        ws: &WorkspaceRef,
        name: &BoxName,
    ) -> Result<PaneRef, EngineError> {
        launch::detach::reattach(self.driver.as_ref(), self.surface.as_ref(), s, ws, name)
    }
}
