//! Tests for the clock adapters.

use std::time::Duration;

use moor_core::seam::clock::Clock;

use super::{parse_test_now, SystemClock, TEST_NOW_VAR};
use crate::env::Env;

const BASE: i64 = 1_760_000_000_000;

#[test]
fn parse_unix_millis() {
    assert_eq!(parse_test_now("1760000000000").unwrap().as_millis(), BASE);
}

#[test]
fn parse_rfc3339_utc_and_offset() {
    assert_eq!(
        parse_test_now("2025-10-09T08:53:20Z").unwrap().as_millis(),
        BASE
    );
    assert_eq!(
        parse_test_now("2025-10-09T16:53:20+08:00")
            .unwrap()
            .as_millis(),
        BASE
    );
}

#[test]
fn parse_rejects_garbage() {
    for s in ["", "-", "abc", "12x4", "2025-13-45", "yesterday"] {
        assert!(parse_test_now(s).is_err(), "{s}");
    }
}

#[test]
fn system_clock_pinned_base_advances() {
    let probe = SystemClock::real();
    let before = probe.instant();
    let env = Env::from_pairs([(TEST_NOW_VAR, "1760000000000")]);
    let c = SystemClock::from_env(&env).unwrap();
    let a = c.now().as_millis();
    let spent = i64::try_from(probe.instant().duration_since(before).as_millis()).unwrap();
    assert!((BASE..=BASE + spent).contains(&a), "{a} after {spent}ms");
    c.sleep(Duration::from_millis(20));
    let b = c.now().as_millis();
    assert!(b >= a + 20, "{a} then {b}");
    let spent = i64::try_from(probe.instant().duration_since(before).as_millis()).unwrap();
    assert!(b <= BASE + spent, "{b} after {spent}ms");
}

#[test]
fn system_clock_without_var_is_real_time() {
    let env = Env::default();
    let c = SystemClock::from_env(&env).unwrap();
    let real = SystemClock::real().now().as_millis();
    assert!((c.now().as_millis() - real).abs() < 3000);
    assert!(c.now().as_millis() > BASE);
}

#[test]
fn system_clock_invalid_var_errors() {
    let env = Env::from_pairs([(TEST_NOW_VAR, "nope")]);
    assert!(SystemClock::from_env(&env).is_err());
}
