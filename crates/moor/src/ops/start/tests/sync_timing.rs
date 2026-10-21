//! How long the checkout sync takes is recorded on the Box.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::proc::Tool;
use moor_core::BoxId;
use serde_json::json;

use super::setup::start_req;
use super::{world_at, World};
use crate::ops::start::run;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

/// A world for `demo` with an author and a clock each Box exec moves on by the next of `steps`.
fn timed_world(steps: Vec<Duration>) -> (World, tempfile::TempDir) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let mut w = world_at(FakeDriver::new(), repo.path());
    w.cx = w.cx.with_clock(clock.clone());
    let ok = |v: &str| Reply::Ok(format!("{v}\n"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.name"], ok("Ann Dev"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.email"], ok("a@x.io"));
    let n = AtomicUsize::new(0);
    w.driver.on_exec(move || {
        if let Some(d) = steps.get(n.fetch_add(1, Ordering::SeqCst)) {
            clock.sleep(*d);
        }
    });
    (w, repo)
}

#[test]
fn synced_records_the_sync_milliseconds() {
    let (w, _repo) = timed_world(vec![Duration::from_secs(7), Duration::from_secs(2)]);
    run(&w.cx, &start_req()).unwrap();
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let synced: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "synced")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(synced, [json!({"ms": 7_000})]);
}
