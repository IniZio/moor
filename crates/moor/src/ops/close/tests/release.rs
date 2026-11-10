//! The release of a close, the step where a provider saves the Box's caches, carries its start
//! and its time in the Box's events.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::DriverError;
use serde_json::{json, Value};

use super::{fx_with, req, run, Fx};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// `driver` in a close fixture whose release takes `took` on the fixture's clock.
fn slow_release(driver: FakeDriver, took: Duration) -> Fx {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let f = fx_with(driver);
    let tick = clock.clone();
    f.driver.on_release(move || tick.sleep(took));
    Fx {
        cx: f.cx.with_clock(clock),
        ..f
    }
}

/// The kinds of the Box's events, oldest first.
fn kinds(f: &Fx) -> Vec<String> {
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

/// The release events of the Box, oldest first, as kind and payload.
fn close_events(f: &Fx) -> Vec<(String, Value)> {
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind.starts_with("release_"))
        .map(|e| (e.kind, e.payload))
        .collect()
}

#[test]
fn a_close_records_the_release_start_and_its_time_before_closed() {
    let f = slow_release(FakeDriver::new(), Duration::from_secs(55));
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(
        close_events(&f),
        [
            ("release_started".to_owned(), json!({})),
            (
                "release_finished".to_owned(),
                json!({"ms": 55_000, "ok": true, "caches_saved": null})
            ),
        ]
    );
    let kinds = kinds(&f);
    let at = |kind: &str| kinds.iter().position(|k| k == kind);
    assert!(at("release_finished") < at("closed"), "{kinds:?}");
}

#[test]
fn a_failed_release_records_its_time_with_ok_false() {
    let failed = DriverError::Failed {
        summary: "x".into(),
    };
    let f = slow_release(
        FakeDriver::new().fail_release_always(failed),
        Duration::from_secs(3),
    );
    run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(
        close_events(&f),
        [
            ("release_started".to_owned(), json!({})),
            (
                "release_finished".to_owned(),
                json!({"ms": 3_000, "ok": false, "caches_saved": null})
            ),
        ]
    );
}
