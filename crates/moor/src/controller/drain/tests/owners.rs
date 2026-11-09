//! The drain tells each op waiting for its Box's turn that the controller is restarting.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::Clock as _;
use moor_core::BoxId;

use super::{Drain, WAIT};
use crate::adapters::clock::SystemClock;
use crate::cx::owners::{Cmd, Owners};
use crate::env::{Cancel, Env};
use crate::Moor;

#[test]
fn an_op_queued_for_its_box_when_the_drain_begins_is_told_the_controller_is_restarting() {
    let clock = SystemClock::real();
    let owners = Arc::new(Owners::new(Arc::new(SystemClock::real())));
    let id = BoxId::new(1);
    let _held = owners.hold(id, Cmd::Steer);
    let sleeper = Arc::clone(&owners);
    let sleep = thread::spawn(move || {
        let deadline = SystemClock::real().instant() + Duration::from_secs(600);
        sleeper
            .turn(id, Cmd::Sleep, &Cancel::default(), deadline)
            .map(drop)
            .map_err(|e| e.error.to_string())
    });
    let queued_by = clock.instant() + WAIT;
    while owners.waiting(id) == 0 {
        assert!(clock.instant() < queued_by, "the sleep never queued");
        thread::sleep(Duration::from_millis(1));
    }
    let drain = Drain::default();
    let env = Env::default().sharing(Arc::clone(&owners));
    drain.drain_owners_with_drain(Moor::owners_drain(&env));
    drain.begin();
    let (got_tx, got) = mpsc::channel();
    thread::spawn(move || got_tx.send(sleep.join().unwrap()).ok());
    let got = got.recv_timeout(WAIT).expect("the waiter was answered");
    assert_eq!(got, Err("controller restarting".to_owned()));
}
