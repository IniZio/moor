//! Tests for the dead intent rule.

use std::time::Duration;

use super::{age, provisioning_dead, PROVISIONING_GRACE};
use crate::domain::decision::Intent;
use crate::name::{BoxHandle, RunId};
use crate::seam::clock::Ts;

fn provisioning() -> Intent {
    Intent::Provisioning { pid: 42 }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn dead_without_handle_past_grace_is_dead() {
    assert!(provisioning_dead(&provisioning(), false, None, secs(121)));
}

#[test]
fn alive_process_is_not_dead() {
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
fn running_intent_is_never_dead() {
    let intent = Intent::Running {
        run: RunId::new(1),
        pid: 42,
    };
    assert!(!provisioning_dead(&intent, false, None, secs(1000)));
}

#[test]
fn setup_intent_is_never_dead() {
    let intent = Intent::Setup { pid: 42 };
    assert!(!provisioning_dead(&intent, false, None, secs(1000)));
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
