//! Tests for the clock fake.
#![expect(
    clippy::disallowed_methods,
    reason = "tests time real elapsed durations"
)]

use std::time::Duration;

use moor_core::seam::clock::{Clock, Ts};

use super::FakeClock;

const BASE: i64 = 1_760_000_000_000;

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
