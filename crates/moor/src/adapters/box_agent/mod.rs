//! The Box agent kit, the one Engine adapter; agent-specific behaviour sits behind an `AgentProfile`.

mod brief;
pub(crate) mod claude;
mod launch;
mod probe;
mod steer;

use std::sync::Arc;

use moor_core::activity::Probes;
use moor_core::seam::agent_profile::{AgentProfile, PromptMatch};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::{Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::Surface;

/// An agent running in a Box, read through the Driver and the Surface.
pub(crate) struct BoxAgent {
    /// Reads the hook file out of the Box.
    driver: Arc<dyn Driver>,
    /// Observes and opens the agent's pane.
    surface: Arc<dyn Surface>,
    /// Parses the agent's hook file and builds its launch.
    profile: Arc<dyn AgentProfile>,
    /// Paces the submit key after typing.
    clock: Arc<dyn Clock>,
}

impl BoxAgent {
    /// A kit over the given Driver, Surface, agent profile and clock.
    pub(crate) fn new(
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        profile: Arc<dyn AgentProfile>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            driver,
            surface,
            profile,
            clock,
        }
    }
}

impl Engine for BoxAgent {
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

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        launch::launch(
            self.driver.as_ref(),
            self.surface.as_ref(),
            self.profile.as_ref(),
            req,
        )
    }

    fn steer(
        &self,
        s: &SessionRef,
        steer: &Steer,
        deadline: Deadline,
    ) -> Result<SteerOutcome, EngineError> {
        steer::steer(
            self.surface.as_ref(),
            self.profile.as_ref(),
            self.clock.as_ref(),
            s,
            steer,
            deadline,
        )
    }
}
