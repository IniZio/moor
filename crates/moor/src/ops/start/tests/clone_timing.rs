//! How long the clone takes is recorded on the Box.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::BoxId;
use serde_json::json;

use super::setup::start_req;
use super::{script_base_yaml, world, World};
use crate::ops::start::run;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// A world for `demo` with a clock each Box exec moves on by the next of `steps`.
fn timed_world(steps: Vec<Duration>) -> World {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let mut w = world(FakeDriver::new());
    script_base_yaml(&w, Some("setup: [a]\n"));
    w.cx = w.cx.with_clock(clock.clone());
    let n = AtomicUsize::new(0);
    w.driver.on_exec(move || {
        if let Some(d) = steps.get(n.fetch_add(1, Ordering::SeqCst)) {
            clock.sleep(*d);
        }
    });
    w
}

#[test]
fn cloned_records_the_clone_milliseconds() {
    let w = timed_world(vec![Duration::from_secs(7), Duration::from_secs(2)]);
    run(&w.cx, &start_req()).unwrap();
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let cloned: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "cloned")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(cloned, [json!({"ms": 7_000})]);
}
