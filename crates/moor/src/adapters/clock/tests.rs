//! Tests for the clock adapters.

use std::time::Duration;

use moor_core::seam::clock::{Clock, Ts};

use super::{parse_test_now, FakeClock, SystemClock, TEST_NOW_VAR};
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
    let env = Env::from_pairs([(TEST_NOW_VAR, "1760000000000")]);
    let c = SystemClock::from_env(&env).unwrap();
    let a = c.now().as_millis();
    assert!((BASE..BASE + 3000).contains(&a), "{a}");
    c.sleep(Duration::from_millis(20));
    assert!(c.now().as_millis() >= a + 20);
}

#[test]
fn system_clock_without_var_is_real_time() {
    let env = Env::from_pairs::<[(&str, &str); 0], _, _>([]);
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

#[test]
fn fake_clock_advances_only_on_sleep() {
    let c = FakeClock::new(Ts::from_millis(BASE));
    let (n0, i0) = (c.now(), c.instant());
    assert_eq!((c.now(), c.instant()), (n0, i0));
    let wall = std::time::Instant::now();
    c.sleep(Duration::from_secs(3600));
    assert!(wall.elapsed() < Duration::from_secs(5));
    assert_eq!(c.now().as_millis(), BASE + 3_600_000);
    assert_eq!(c.instant() - i0, Duration::from_secs(3600));
}
