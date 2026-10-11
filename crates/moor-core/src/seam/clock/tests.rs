//! Tests for `Ts`.

use super::Ts;
use std::time::Duration;

#[test]
fn millis_round_trip() {
    assert_eq!(Ts::from_millis(1_234).as_millis(), 1_234);
    assert_eq!(Ts::from_millis(-5).as_millis(), -5);
}

#[test]
fn saturating_add_adds_and_saturates() {
    let t = Ts::from_millis(1_000);
    assert_eq!(
        t.saturating_add(Duration::from_millis(250)).as_millis(),
        1_250
    );
    assert_eq!(
        Ts::from_millis(i64::MAX - 1).saturating_add(Duration::from_secs(10)),
        Ts::from_millis(i64::MAX)
    );
    assert_eq!(t.saturating_add(Duration::MAX), Ts::from_millis(i64::MAX));
}

#[test]
fn ordering_follows_millis() {
    assert!(Ts::from_millis(1) < Ts::from_millis(2));
    assert_eq!(Ts::from_millis(3), Ts::from_millis(3));
}

#[test]
fn serde_is_transparent() {
    let t = Ts::from_millis(42);
    assert_eq!(serde_json::to_string(&t).unwrap(), "42");
    assert_eq!(serde_json::from_str::<Ts>("42").unwrap(), t);
}
