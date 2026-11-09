//! Ops fired at one Box from several threads apply in the order they reached its owner.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::Clock as _;
use moor_core::{BoxName, BoxReq, CloseReq};

use super::{fx, text, Fx};
use crate::adapters::clock::SystemClock;
use crate::cx::owners::{Cmd, Owners};
use crate::cx::Cx;
use crate::ops::{close, sleep, steer};
use crate::testing::proc::ScriptedProc;

/// Another request's context on `f`'s Box, admitted through `owners`.
fn peer(f: &Fx, owners: &Arc<Owners>) -> Cx {
    Cx::fake(
        f.dir.path(),
        f.driver.clone(),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(f.engine.clone())
    .with_owners(Arc::clone(owners))
}

/// Waits until `n` ops are queued on `f`'s Box.
fn queued(f: &Fx, owners: &Owners, n: usize) {
    let clock = SystemClock::real();
    let deadline = clock.instant() + Duration::from_secs(10);
    while owners.waiting(f.id) < n {
        assert!(clock.instant() < deadline, "{n} ops never queued");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn steer_sleep_and_a_forced_close_fired_together_apply_in_arrival_order() {
    let f = fx();
    let owners = Arc::new(Owners::new(Arc::new(SystemClock::real())));
    let name = || BoxName::parse_allow_bare("t").unwrap();
    let held = owners.hold(f.id, Cmd::Settle);
    let (steering, sleeping, closing) = (peer(&f, &owners), peer(&f, &owners), peer(&f, &owners));
    let steered = thread::spawn(move || steer::run(&steering, &text("hi")).map(drop));
    queued(&f, &owners, 1);
    let slept =
        thread::spawn(move || sleep::run(&sleeping, &BoxReq { box_name: name() }).map(drop));
    queued(&f, &owners, 2);
    let closed = thread::spawn(move || {
        let req = CloseReq {
            target: name().into(),
            force: true,
        };
        close::run(&closing, &req).map(drop)
    });
    queued(&f, &owners, 3);
    drop(held);
    for (op, done) in [("steer", steered), ("sleep", slept), ("close", closed)] {
        let got = done.join().unwrap();
        assert!(got.is_ok(), "{op}: {:?}", got.err().map(|e| e.error));
    }
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    let at = |kind: &str| events.iter().find(|e| e.kind == kind).map(|e| e.id);
    let (steered, asleep, closed) = (at("steered"), at("asleep"), at("closed"));
    assert!(
        steered.is_some() && steered < asleep && asleep < closed,
        "{events:?}"
    );
}
