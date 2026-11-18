//! A machine's link that is down when the agent install starts, or drops while it runs: the
//! install is asked again once the link is back, so a controller deploy does not fail setup.

use moor_core::seam::driver::DriverError;

use super::agent_install::{install_world, installs, prompt_req, SCRIPT};
use super::state::WORKING;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// What the machine adapter says while the controller holds no link to the machine.
fn unlinked() -> DriverError {
    DriverError::Unavailable("machine 1 is not linked".to_owned())
}

#[test]
fn an_install_that_starts_while_the_link_is_down_waits_for_the_relink() {
    let driver = FakeDriver::new()
        .exec_fails_once_when(SCRIPT, unlinked())
        .exec_fails_once_when(SCRIPT, unlinked());
    let (w, engine) = install_world("setup: [a]\n", None, true, driver);
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(installs(&w).len(), 3, "two asks while down, one after");
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn an_install_cut_mid_run_is_asked_again_once_the_link_is_back() {
    let cut = DriverError::Transport {
        stderr_tail: "the stream ended before the command did".to_owned(),
    };
    let driver = FakeDriver::new().exec_fails_once_when(SCRIPT, cut);
    let (w, engine) = install_world("setup: [a]\n", None, true, driver);
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(installs(&w).len(), 2);
    assert_eq!(engine.launches().len(), 1);
}
