//! The Box agent kit, the one Engine adapter; agent-specific behaviour sits behind an `AgentAdapter`.

mod agent_env;
mod brief;
pub(crate) mod claude;
#[allow(
    dead_code,
    reason = "the launch script calls the writers in a later change"
)]
mod codex;
mod launch;

pub(crate) use launch::detach::relay_argv;
pub(crate) use launch::holder::holder;
#[cfg(test)]
pub(crate) use launch::holder::TMUX;
mod probe;
#[allow(
    dead_code,
    reason = "the launch script calls the writers in a later change"
)]
mod profile_writer;
mod proxy_env;
mod steer;
mod transcript;

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{Probes, WaitCheck};
use moor_core::config::AgentHolder;
use moor_core::name::{BoxName, BoxPath};
use moor_core::seam::agent_adapter::{AgentAdapter, AgentInstall, PromptMatch};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::{
    AgentFate, Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, Surface, WorkspaceRef};

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
    /// Holds the terminal of each new agent; a running one keeps the holder its session records.
    holder: AgentHolder,
}

impl BoxAgent {
    /// A kit over the given Driver, Surface, agent adapter and clock.
    pub(crate) fn new(
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        profile: Arc<dyn AgentAdapter>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            driver,
            surface,
            profile,
            clock,
            holder: AgentHolder::default(),
        }
    }

    /// The same kit with the terminal of each agent it launches held by `kind`.
    #[must_use]
    pub(crate) const fn holding(mut self, kind: AgentHolder) -> Self {
        self.holder = kind;
        self
    }
}

impl Engine for BoxAgent {
    fn relay_argv(&self, kind: AgentHolder, connect: &[String]) -> Option<Vec<String>> {
        Some(relay_argv(holder(kind), connect))
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

    fn check_wait(&self, s: &SessionRef, deadline: Deadline) -> WaitCheck {
        probe::check_wait(self.driver.as_ref(), self.profile.as_ref(), s, deadline)
    }

    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch> {
        self.profile.first_run_prompt(tail)
    }

    fn hooks_quiet_during_first_run(&self) -> bool {
        self.profile.hooks_quiet_during_first_run()
    }

    fn agent_install(&self) -> Option<AgentInstall> {
        launch::holder::with_agent(holder(self.holder), self.profile.install())
    }

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        launch::launch(
            self.driver.as_ref(),
            self.surface.as_ref(),
            self.profile.as_ref(),
            req,
            self.holder,
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
        launch::detach::stop(holder(s.holder), self.driver.as_ref(), s)
    }

    fn agent_fate(&self, s: &SessionRef, deadline: Deadline) -> Result<AgentFate, EngineError> {
        launch::detach::fate(holder(s.holder), self.driver.as_ref(), s, deadline)
    }

    fn reattach(
        &self,
        s: &SessionRef,
        ws: &WorkspaceRef,
        name: &BoxName,
    ) -> Result<PaneRef, EngineError> {
        launch::detach::reattach(
            holder(s.holder),
            self.driver.as_ref(),
            self.surface.as_ref(),
            s,
            ws,
            name,
        )
    }
}
