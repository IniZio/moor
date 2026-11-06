//! The runner's links broker as the crabbox adapter's pool hosts and the ops' machine links.

use std::path::Path;
use std::sync::Arc;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::status::Status;
use moor_core::seam::clock::Clock;

use crate::adapters::crabbox::PoolHosts;
use crate::cx::MachineLinks;
use crate::proxy::links::RunnerLinks;
use crate::store::Store;

/// The links broker of the runner under `state_dir`, routed by the links `store` records.
pub(super) fn under(state_dir: &Path, store: Option<Store>, clock: Arc<dyn Clock>) -> Arc<Links> {
    Arc::new(Links {
        runner: RunnerLinks::under(state_dir),
        store,
        clock,
    })
}

/// The runner's links broker, seen from the crabbox adapter and from ops.
pub(super) struct Links {
    /// The broker of the one runner this controller reaches.
    runner: RunnerLinks,
    /// The recorded links; none outside a controller, where the runner answers for itself.
    store: Option<Store>,
    /// The time a link's last report is measured against.
    clock: Arc<dyn Clock>,
}

impl Links {
    /// The broker to reach machine `machine_id` through: refused unless a runner holds its link.
    fn route(&self, machine_id: i64) -> Result<&RunnerLinks, String> {
        let Some(store) = &self.store else {
            return Ok(&self.runner);
        };
        match store.machine_runner(machine_id, self.clock.now()) {
            Ok(Some(_)) => Ok(&self.runner),
            Ok(None) => Err(format!("machine {machine_id} is not linked")),
            Err(e) => Err(format!("cannot read machine {machine_id}'s link: {e}")),
        }
    }
}

impl PoolHosts for Links {
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
        boot: &Boot,
    ) -> Result<Answer, String> {
        self.route(machine_id)?
            .lease(machine_id, verb, slug, key, boot)
    }

    fn dial_command(&self, machine_id: i64, slug: &str) -> String {
        self.runner.dial_command(machine_id, slug)
    }
}

impl MachineLinks for Links {
    fn status(&self, machine_id: i64) -> Result<Status, String> {
        self.runner.status(machine_id)
    }
}
