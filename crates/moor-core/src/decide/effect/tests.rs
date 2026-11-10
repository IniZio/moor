//! Tests for the dead effect rule.

use std::time::Duration;

use super::{age, provisioning_dead, setup_dead, PROVISIONING_GRACE};
use crate::domain::decision::Effect;
use crate::name::{BoxHandle, RunId};
use crate::seam::clock::Ts;

fn provisioning() -> Effect {
    Effect::Acquire
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn idle_without_handle_past_grace_is_dead() {
    assert!(provisioning_dead(&provisioning(), false, None, secs(121)));
}

#[test]
fn an_op_in_flight_is_not_dead() {
    assert!(!provisioning_dead(&provisioning(), true, None, secs(121)));
}

#[test]
fn recorded_handle_is_not_dead() {
    let handle = BoxHandle::new("box-1");
    assert!(!provisioning_dead(
        &provisioning(),
        false,
        Some(&handle),
        secs(121)
    ));
}

#[test]
fn exactly_grace_is_not_dead() {
    assert_eq!(PROVISIONING_GRACE, secs(120));
    assert!(!provisioning_dead(
        &provisioning(),
        false,
        None,
        PROVISIONING_GRACE
    ));
}

#[test]
fn inside_grace_is_not_dead() {
    assert!(!provisioning_dead(&provisioning(), false, None, secs(119)));
}

#[test]
fn a_run_is_never_dead() {
    let effect = Effect::Run { run: RunId::new(1) };
    assert!(!provisioning_dead(&effect, false, None, secs(1000)));
}

#[test]
fn a_setup_is_never_provisioning_dead() {
    let effect = Effect::Setup;
    assert!(!provisioning_dead(&effect, false, None, secs(1000)));
}

#[test]
fn setup_with_no_op_in_flight_is_dead() {
    assert!(setup_dead(&Effect::Setup, false));
}

#[test]
fn setup_with_an_op_in_flight_is_not_dead() {
    assert!(!setup_dead(&Effect::Setup, true));
}

#[test]
fn provisioning_and_running_are_never_setup_dead() {
    let running = Effect::Run { run: RunId::new(1) };
    assert!(!setup_dead(&provisioning(), false));
    assert!(!setup_dead(&running, false));
}

#[test]
fn age_of_earlier_now_is_zero() {
    assert_eq!(
        age(Ts::from_millis(5_000), Ts::from_millis(1_000)),
        Duration::ZERO
    );
}

#[test]
fn age_is_the_difference() {
    assert_eq!(
        age(Ts::from_millis(1_000), Ts::from_millis(3_500)),
        Duration::from_millis(2_500)
    );
}
