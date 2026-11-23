//! The drain tells each op waiting for its Box's turn that the controller is restarting.

use std::io::Cursor;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::Clock as _;
use moor_core::BoxId;

use super::{Admitted, Drain, WAIT};
use crate::adapters::clock::SystemClock;
use crate::controller::conn::serve;
use crate::cx::owners::{Cmd, Owners};
use crate::env::{Cancel, Env};
use crate::Moor;
use moor_core::controller::PROTOCOL;
use moor_core::Request;
use serde_json::{json, Value};

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

#[test]
fn a_command_refused_for_the_drain_while_waiting_for_its_box_gets_restarting() {
    let owners = Owners::new(Arc::new(SystemClock::real()));
    owners.drain();
    let deadline = SystemClock::real().instant() + Duration::from_secs(60);
    let input = format!(
        "{}\n{}\n",
        json!({"frame": "hello", "protocol": PROTOCOL}),
        json!({"frame": "request", "id": "7", "request": Request::Version {}})
    );
    let mut out = Vec::new();
    serve(
        Cursor::new(input.into_bytes()),
        &mut out,
        &Admitted::unwatched(),
        |_, _, _| {
            let refused = owners
                .call(
                    BoxId::new(1),
                    Cmd::Steer,
                    &Cancel::default(),
                    deadline,
                    |_| Ok(()),
                )
                .unwrap_err();
            Some(refused.into_response("steer", vec![]))
        },
        |_, _, _, _, _| {},
    );
    let frames: Vec<Value> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json frame"))
        .collect();
    let last = frames.last().expect("a frame");
    assert_eq!(last["frame"], "error", "{frames:?}");
    assert_eq!(last["kind"], "restarting", "{frames:?}");
    assert_eq!(last["id"], "7", "{frames:?}");
}
