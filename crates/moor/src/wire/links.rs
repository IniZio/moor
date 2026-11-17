//! The runners that hold machines' links, as the ops' machine links and the machine driver's reach.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::reach::Runner;
use moor_client::link::status::Status;
use moor_core::seam::clock::Clock;

use crate::adapters::machine::Reach;
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

impl MachineLinks for Runners {
    fn status(&self, machine_id: i64) -> Result<Status, String> {
        Self::status(self, machine_id)
    }
}

impl Reach for Runners {
    fn runner(&self, machine_id: i64) -> Result<Runner, String> {
        Self::runner(self, machine_id)
    }
}

/// Reaches no runner, for a controller with no state directory to record links in.
struct NoRunners;

impl Reach for NoRunners {
    fn runner(&self, machine_id: i64) -> Result<Runner, String> {
        Err(format!(
            "machine {machine_id} is not linked: there is no state directory"
        ))
    }
}

/// How the machine driver reaches runners: through `links` when the controller has them.
pub(super) fn reach(links: Option<Arc<Runners>>) -> Arc<dyn Reach> {
    links.map_or_else(
        || Arc::new(NoRunners) as Arc<dyn Reach>,
        |l| l as Arc<dyn Reach>,
    )
}

/// This moor binary, which a Box terminal runs `box-term` of.
pub(super) fn moor_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("moor"))
}
