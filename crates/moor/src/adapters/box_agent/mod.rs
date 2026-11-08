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

use moor_core::activity::{Probe, WaitCheck};
use moor_core::name::BoxPath;
use moor_core::profile::Manifest;
use moor_core::seam::agent_adapter::{AgentAdapter, AgentInstall, PromptMatch};
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::{
    Engine, EngineError, GitSource, LaunchReq, SessionRef, Steer, SteerOutcome,
};
use moor_core::seam::proc::Deadline;

/// An agent running in a Box, held and read by herdr-core there through the Driver.
pub(crate) struct BoxAgent {
    /// Reaches the Box.
    driver: Arc<dyn Driver>,
    /// Reads the agent's note and builds its launch.
    profile: Arc<dyn AgentAdapter>,
}

impl BoxAgent {
    /// A kit over the given Driver and agent adapter.
    pub(crate) fn new(driver: Arc<dyn Driver>, profile: Arc<dyn AgentAdapter>) -> Self {
        Self { driver, profile }
    }
}

impl Engine for BoxAgent {
    fn relay_argv(&self, connect: &[String]) -> Option<Vec<String>> {
        Some(relay_argv(connect))
    }

    fn probe(&self, s: &SessionRef, deadline: Deadline) -> Probe {
        probe::probe(self.driver.as_ref(), self.profile.as_ref(), s, deadline)
    }

    fn check_wait(&self, s: &SessionRef, deadline: Deadline) -> WaitCheck {
        probe::check_wait(self.driver.as_ref(), self.profile.as_ref(), s, deadline)
    }

    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch> {
        self.profile.first_run_prompt(tail)
    }

    fn agent_install(&self) -> Option<AgentInstall> {
        Some(launch::holder::with_agent(self.profile.install()))
    }

    fn profile_install(&self, manifest: &Manifest) -> Option<String> {
        profile_writer::install_script(manifest)
    }

    fn profile_sources(&self, manifest: &Manifest) -> Vec<GitSource> {
        profile_writer::all_sources(manifest)
    }

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        launch::launch(self.driver.as_ref(), self.profile.as_ref(), req)
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
            self.driver.as_ref(),
            self.profile.as_ref(),
            s,
            steer,
            deadline,
        )?;
        let _ = self.driver.touch(&s.handle);
        Ok(sent)
    }

    fn transcript(
        &self,
        s: &SessionRef,
        dest: &Path,
        deadline: Deadline,
    ) -> Result<(), EngineError> {
        transcript::save(
            self.driver.as_ref(),
            self.profile.as_ref(),
            s,
            dest,
            deadline,
        )
    }

    fn stop(&self, s: &SessionRef) -> Result<(), EngineError> {
        launch::detach::stop(self.driver.as_ref(), s)
    }
}
