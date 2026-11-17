//! A Box goes to the driver of the node it lives on, whichever call names it.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::box_key::{BoxKey, ControllerId, PoolSeat};
use moor_core::name::{BoxHandle, BoxId, ProviderId};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;

use super::Routed;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::machine::{Machines, Reach};
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::env::Env;

/// Reaches no runner.
struct Nowhere;

impl Reach for Nowhere {
    fn runner(&self, machine_id: i64) -> Result<moor_client::link::reach::Runner, String> {
        Err(format!("machine {machine_id} is not linked"))
    }
}

fn routed() -> Routed {
    let env = Env::from_pairs([("HOME", "/nonexistent")]);
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(&env, &ToolPaths::default()));
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::from_env(&env).unwrap());
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    Routed {
        crabbox: Crabbox::new(proc, clock, rng),
        machines: Machines::new(Arc::new(Nowhere), PathBuf::from("moor")),
    }
}

/// Whether `driver` is the routed machine driver.
fn is_machines(routed: &Routed, driver: &dyn Driver) -> bool {
    std::ptr::addr_eq(driver, &raw const routed.machines)
}

#[test]
fn a_machine_handle_goes_to_machines_and_any_other_handle_to_crabbox() {
    let routed = routed();
    let seat = PoolSeat {
        machine: 7,
        controller: ControllerId::parse("1a2b3c4d").unwrap(),
        inc: 1,
    };
    let ours = BoxHandle::new(BoxKey::new(BoxId::new(41)).machine_handle(&seat));
    assert!(is_machines(&routed, routed.of(&ours)));
    for theirs in [
        "external/cbx_1",
        "cloudhypervisor@hv1/7/moor-1a2b3c4d-41",
        "m7",
    ] {
        let handle = BoxHandle::new(theirs);
        assert!(!is_machines(&routed, routed.of(&handle)), "{theirs}");
    }
}

#[test]
fn the_runtime_of_a_machine_that_provides_boxes_goes_to_machines() {
    let routed = routed();
    for runtime in ["boxes", "boxes@local"] {
        let provider = ProviderId::new(runtime);
        assert!(
            is_machines(&routed, routed.of_provider(&provider)),
            "{runtime}"
        );
    }
    for runtime in ["external", "cloudhypervisor@hv1", "boxes-not"] {
        let provider = ProviderId::new(runtime);
        assert!(
            !is_machines(&routed, routed.of_provider(&provider)),
            "{runtime}"
        );
    }
}
