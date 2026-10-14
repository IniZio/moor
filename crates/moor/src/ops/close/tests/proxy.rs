use std::time::Duration;

use moor_core::domain::task::TaskState;
use moor_core::seam::proc::Signal;

use super::{fx, req, run, Fx};
use crate::ops::common::proxy::{new_nonce, LEASE};

const PID: u32 = 4242;

/// A proxy row with a supervisor pid; its lease is held now.
fn with_proxy(f: &Fx) {
    let store = f.cx.store().unwrap();
    let nonce = new_nonce(f.cx.rng());
    let _ = store.reserve(f.id, &nonce, f.cx.rng(), LEASE).unwrap();
    store.claim_lease(f.id, "sup", LEASE).unwrap();
    rusqlite::Connection::open(f.dir.path().join("moor.db"))
        .unwrap()
        .execute(
            "UPDATE proxy SET ready = 1, host_port = 4000, pid = ? WHERE task = ?",
            [i64::from(PID), f.id.get()],
        )
        .unwrap();
}

fn has_row(f: &Fx) -> bool {
    f.cx.store().unwrap().proxy(f.id).unwrap().is_some()
}

#[test]
fn live_lease_signals_the_group_and_removes_the_row() {
    let f = fx();
    with_proxy(&f);

    let v = run(&f.cx, &req(false)).unwrap();

    assert_eq!(v.task.state, TaskState::Closed);
    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!has_row(&f));
}

#[test]
fn stale_lease_sends_no_signal_and_removes_the_row() {
    let f = fx();
    with_proxy(&f);
    f.cx.clock().sleep(LEASE + Duration::from_secs(1));

    run(&f.cx, &req(false)).unwrap();

    assert!(f.proc.signals().is_empty());
    assert!(!has_row(&f));
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn no_row_is_a_no_op() {
    let f = fx();

    run(&f.cx, &req(false)).unwrap();

    assert!(f.proc.signals().is_empty());
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn a_supervisor_that_ignores_the_signal_only_warns() {
    let f = fx();
    with_proxy(&f);
    let nonce = f.cx.store().unwrap().proxy(f.id).unwrap().unwrap().nonce;
    let secret = String::from_utf8(nonce.expose().to_vec()).unwrap();

    let v = run(&f.cx, &req(false)).unwrap();

    assert_eq!(v.task.state, TaskState::Closed);
    assert!(!has_row(&f));
    let warnings = f.cx.take_warnings();
    let stops = warnings
        .iter()
        .filter(|w| w.code == "proxy_stop_failed")
        .count();
    assert_eq!(stops, 1);
    assert!(warnings.iter().all(|w| !w.message.contains(&secret)));
}
