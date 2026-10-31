//! The runner's links broker as the crabbox adapter's pool hosts and the ops' machine links.

use std::path::Path;
use std::sync::Arc;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::status::Status;

use crate::adapters::crabbox::PoolHosts;
use crate::cx::MachineLinks;
use crate::proxy::links::RunnerLinks;

/// The links broker of the runner under `state_dir`.
pub(super) fn under(state_dir: &Path) -> Arc<Links> {
    Arc::new(Links(RunnerLinks::under(state_dir)))
}

/// The runner's links broker, seen from the crabbox adapter and from ops.
pub(super) struct Links(RunnerLinks);

impl PoolHosts for Links {
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
        boot: &Boot,
    ) -> Result<Answer, String> {
        self.0.lease(machine_id, verb, slug, key, boot)
    }

    fn dial_command(&self, machine_id: i64, slug: &str) -> String {
        self.0.dial_command(machine_id, slug)
    }
}

impl MachineLinks for Links {
    fn status(&self, machine_id: i64) -> Result<Status, String> {
        self.0.status(machine_id)
    }
}
