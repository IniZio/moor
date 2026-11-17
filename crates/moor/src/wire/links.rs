//! The runners that hold machines' links as the crabbox adapter's pool hosts and the ops'
//! machine links.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::reach::Runner;
use moor_client::link::reach::StdConn;
use moor_client::link::status::Status;
use moor_core::seam::clock::Clock;

use crate::adapters::crabbox::{PoolHosts, PoolPorts};
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

    fn dial_command(&self, machine_id: i64, slug: &str) -> Result<String, String> {
        Self::dial_command(self, machine_id, slug)
    }
}

impl PoolPorts for Runners {
    fn port_listens(&self, machine_id: i64, slug: &str, port: u16) -> Result<bool, String> {
        let runner = self.runner(machine_id)?;
        crate::proxy::links::listens(&runner, machine_id, slug, port)
    }

    fn port_conn(&self, machine_id: i64, slug: &str, port: u16) -> Result<StdConn, String> {
        let runner = self.runner(machine_id)?;
        crate::proxy::links::port(&runner, machine_id, slug, port)
    }
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
