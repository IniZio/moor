//! What a launch tells the contract: whether it serves a run, and the version a fresh session read.

use moor_core::config::BoxSettings;
use moor_core::contract::{contract, Stamp};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ContractEvent;
use moor_core::seam::engine::{LaunchReq, RunLaunch};
use moor_core::seam::proc::Tool;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::{RunOfBox, Store};

/// The env var that carries a run page's address into its Box.
const RUN_URL: &str = "MOOR_RUN_URL";

/// Marks `launch` as a run's when its start said so or Box `id`'s latest run is open, with the
/// run page's address from `cfg` when the controller knows it.
///
/// # Errors
/// A store failure.
pub(super) fn situate(
    store: &Store,
    id: BoxId,
    launch: &mut LaunchReq,
    cfg: &BoxSettings,
) -> Result<(), CommandError> {
    if launch.run.is_none() && store.run_of_box(id)? == RunOfBox::Open {
        launch.run = Some(RunLaunch { link: None });
    }
    if let Some(run) = launch.run.as_mut() {
        run.link = cfg.env.get(RUN_URL).cloned();
    }
    Ok(())
}

/// Records the contract a fresh session of a work Box or an agent place read.
///
/// # Errors
/// A store failure.
pub(super) fn given(store: &Store, id: BoxId) -> Result<(), CommandError> {
    let Ok(contract) = contract() else {
        return Ok(());
    };
    let Stamp { version, sha256 } = contract.stamp();
    let event = ContractEvent::ContractGiven {
        version,
        sha256,
        harness: Tool::Agent.program().to_owned(),
    };
    store.apply(Decision::new(id).event(ExtraEvent::Contract(event)))?;
    Ok(())
}
