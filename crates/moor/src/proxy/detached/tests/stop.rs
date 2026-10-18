//! Stopping a Task's proxy supervisor through the adapter.

use std::time::Duration;

use moor_core::seam::clock::Clock as _;
use moor_core::seam::proc::Signal;
use moor_core::seam::proxy_host::{ProxyHost, ProxyHostError};

use super::{fx, Fx};
use crate::proxy::detached::LEASE;

const PID: u32 = 4242;

const BOOT: &str = "boot-a";

/// A proxy row with a supervisor pid; its lease is held now and it was written in this boot.
fn with_proxy(f: &Fx) {
    f.proc.with_boot_id(BOOT);
    proxy_row(f);
}

/// The same row on a host that reports no boot id.
fn proxy_row(f: &Fx) {
    f.leased_row();
    f.exec(&format!(
        "UPDATE proxy SET ready = 1, host_port = 4000, pid = {PID}, boot_id = '{BOOT}'"
    ));
}

#[test]
fn live_lease_signals_the_group_and_removes_the_row() {
    let f = fx();
    with_proxy(&f);

    f.sup.stop(f.id).unwrap_err();

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!f.has_row());
}

#[test]
fn a_supervisor_that_exits_on_the_signal_ends_the_wait_early() {
    let f = fx();
    with_proxy(&f);
    let db = f.dir.path().join(crate::store::STORE_FILE);
    let id = f.id.get();
    f.proc.on_signal(move |_, _| {
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute("DELETE FROM proxy WHERE box = ?", [id])
            .unwrap();
    });
    let start = f.clock.instant();

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!f.has_row());
    let waited = f.clock.instant().saturating_duration_since(start);
    assert!(waited < Duration::from_secs(1), "waited {waited:?}");
}

#[test]
fn stale_lease_sends_no_signal_and_removes_the_row() {
    let f = fx();
    with_proxy(&f);
    f.clock.sleep(LEASE + Duration::from_secs(1));

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(f.proc.signals().is_empty());
    assert!(!f.has_row());
}

#[test]
fn no_row_is_a_no_op() {
    let f = fx();

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(f.proc.signals().is_empty());
}

#[test]
fn a_supervisor_that_ignores_the_signal_is_reported_incomplete() {
    let f = fx();
    with_proxy(&f);
    let row = f.store.proxy(f.id).unwrap().unwrap();
    let secret = String::from_utf8(row.nonce.expose().to_vec()).unwrap();

    let err = f.sup.stop(f.id).unwrap_err();

    assert_eq!(
        err,
        ProxyHostError::Incomplete("the proxy supervisor did not exit within 3s".into())
    );
    assert!(!f.has_row());
    assert!(!err.to_string().contains(&secret));
}

#[test]
fn a_vanished_group_is_stopped_without_an_error() {
    let f = fx();
    f.proc.groups_gone();
    with_proxy(&f);

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
    assert!(!f.has_row());
}

#[test]
fn a_row_from_another_boot_sends_no_signal_and_is_removed() {
    let f = fx();
    with_proxy(&f);
    f.proc.with_boot_id("boot-b");

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(f.proc.signals().is_empty());
    assert!(!f.has_row());
}

#[test]
fn a_row_without_a_boot_id_is_not_signalled_when_the_host_has_one() {
    let f = fx();
    with_proxy(&f);
    f.exec("UPDATE proxy SET boot_id = NULL");

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(f.proc.signals().is_empty());
    assert!(!f.has_row());
}

#[test]
fn a_host_without_a_boot_id_still_signals_a_live_lease() {
    let f = fx();
    proxy_row(&f);

    f.sup.stop(f.id).unwrap_err();

    assert_eq!(f.proc.signals(), [(PID, Signal::Term)]);
}

#[test]
fn a_heartbeat_far_in_the_future_sends_no_signal() {
    let f = fx();
    with_proxy(&f);
    let ahead = f.clock.now().as_millis() + 60_000;
    f.exec(&format!("UPDATE proxy SET lease_heartbeat = {ahead}"));

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(f.proc.signals().is_empty());
    assert!(!f.has_row());
}

fn ca_file(f: &Fx) -> std::path::PathBuf {
    let dir = crate::proxy::nono::ca::ca_dir(f.dir.path(), f.id);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ca.key");
    std::fs::write(&file, b"key").unwrap();
    file
}

#[test]
fn stopping_removes_the_ca_directory_with_the_row() {
    let f = fx();
    with_proxy(&f);
    let file = ca_file(&f);
    f.proc.groups_gone();

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(!f.has_row());
    assert!(!file.parent().unwrap().exists());
}

#[test]
fn a_stale_lease_stop_removes_the_ca_directory() {
    let f = fx();
    with_proxy(&f);
    let file = ca_file(&f);
    f.clock.sleep(LEASE + Duration::from_secs(1));

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(!file.parent().unwrap().exists());
}

#[test]
fn a_supervisor_that_outlasts_the_wait_keeps_its_ca_directory() {
    let f = fx();
    with_proxy(&f);
    let file = ca_file(&f);

    f.sup.stop(f.id).unwrap_err();

    assert!(file.exists());
}

#[test]
fn stopping_without_a_ca_directory_succeeds() {
    let f = fx();
    with_proxy(&f);
    f.proc.groups_gone();

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(!f.has_row());
}

#[test]
fn a_retried_stop_without_a_row_removes_the_ca_directory() {
    let f = fx();
    let file = ca_file(&f);
    assert!(!f.has_row());

    assert_eq!(f.sup.stop(f.id), Ok(()));

    assert!(!file.parent().unwrap().exists());
    assert_eq!(f.sup.stop(f.id), Ok(()));
}
