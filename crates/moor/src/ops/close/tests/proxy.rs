use std::time::Duration;

use moor_core::domain::task::TaskState;
use moor_core::seam::proc::Signal;

use super::{fx, req, run, Fx};
use crate::ops::common::proxy::{new_nonce, LEASE};

const PID: u32 = 4242;

const BOOT: &str = "boot-a";

/// A proxy row with a supervisor pid; its lease is held now and it was written in this boot.
fn with_proxy(f: &Fx) {
    f.proc.with_boot_id(BOOT);
    proxy_row(f);
}

/// The same row on a host that reports no boot id.
fn proxy_row(f: &Fx) {
    let store = f.cx.store().unwrap();
    let nonce = new_nonce(f.cx.rng());
    let _ = store.reserve(f.id, &nonce, f.cx.rng(), LEASE).unwrap();
    store.claim_lease(f.id, "sup", LEASE).unwrap();
    rusqlite::Connection::open(f.dir.path().join("moor.db"))
        .unwrap()
        .execute(
            "UPDATE proxy SET ready = 1, host_port = 4000, pid = ?, boot_id = ? WHERE task = ?",
            rusqlite::params![PID, BOOT, f.id.get()],
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
fn a_supervisor_that_exits_on_the_signal_ends_the_wait_early() {
    let f = fx();
    with_proxy(&f);
    let db = f.dir.path().join("moor.db");
    let id = f.id.get();
    f.proc.on_signal(move |_, _| {
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute("DELETE FROM proxy WHERE task = ?", [id])
            .unwrap();
    });
    let start = f.cx.clock().instant();

    run(&f.cx, &req(false)).unwrap();

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!has_row(&f));
    assert!(f.cx.take_warnings().is_empty());
    let waited = f.cx.clock().instant().saturating_duration_since(start);
    assert!(waited < Duration::from_secs(1), "waited {waited:?}");
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

#[test]
fn a_vanished_group_is_stopped_without_a_warning() {
    let f = fx();
    f.proc.groups_gone();
    with_proxy(&f);

    run(&f.cx, &req(false)).unwrap();

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!has_row(&f));
    assert!(f.cx.take_warnings().is_empty());
}

fn exec(f: &Fx, sql: &str) {
    rusqlite::Connection::open(f.dir.path().join("moor.db"))
        .unwrap()
        .execute(sql, [])
        .unwrap();
}

#[test]
fn a_row_from_another_boot_sends_no_signal_and_is_removed() {
    let f = fx();
    with_proxy(&f);
    f.proc.with_boot_id("boot-b");

    run(&f.cx, &req(false)).unwrap();

    assert!(f.proc.signals().is_empty());
    assert!(!has_row(&f));
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn a_row_without_a_boot_id_is_not_signalled_when_the_host_has_one() {
    let f = fx();
    with_proxy(&f);
    exec(&f, "UPDATE proxy SET boot_id = NULL");

    run(&f.cx, &req(false)).unwrap();

    assert!(f.proc.signals().is_empty());
    assert!(!has_row(&f));
}

#[test]
fn a_host_without_a_boot_id_still_signals_a_live_lease() {
    let f = fx();
    proxy_row(&f);

    run(&f.cx, &req(false)).unwrap();

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
}

#[test]
fn a_heartbeat_far_in_the_future_sends_no_signal() {
    let f = fx();
    with_proxy(&f);
    let ahead = f.cx.clock().now().as_millis() + 60_000;
    exec(&f, &format!("UPDATE proxy SET lease_heartbeat = {ahead}"));

    run(&f.cx, &req(false)).unwrap();

    assert!(f.proc.signals().is_empty());
    assert!(!has_row(&f));
}
