//! The runners that hold machines' links as the crabbox adapter's pool hosts and the ops'
//! machine links.

use std::path::Path;
use std::sync::Arc;

use moor_client::link::lease::{Answer, Boot, CheckpointAct, Verb};
use moor_client::link::status::Status;
use moor_core::seam::clock::Clock;

use crate::adapters::crabbox::PoolHosts;
use crate::cx::MachineLinks;
use crate::proxy::links::Runners;
use crate::store::Store;

/// The runners of the controller under `state_dir`, routed by the links `store` records and
/// reached across the network with `key`.
pub(super) fn under(
    state_dir: &Path,
    store: Option<Store>,
    clock: Arc<dyn Clock>,
    key: Option<&str>,
) -> Arc<Runners> {
    Arc::new(Runners::new(state_dir, store, clock, key))
}

impl PoolHosts for Runners {
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
        boot: &Boot,
    ) -> Result<Answer, String> {
        Self::lease(self, machine_id, verb, slug, key, boot)
    }

    fn checkpoint(&self, machine_id: i64, act: CheckpointAct) -> Result<Answer, String> {
        Self::checkpoint(self, machine_id, act)
    }

    fn dial_command(&self, machine_id: i64, slug: &str) -> Result<String, String> {
        Self::dial_command(self, machine_id, slug)
    }
}

impl MachineLinks for Runners {
    fn status(&self, machine_id: i64) -> Result<Status, String> {
        Self::status(self, machine_id)
    }
}
