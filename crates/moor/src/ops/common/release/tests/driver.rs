//! What the driver's answer does to the lease rows of a Box release.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::time::Duration;

use moor_core::seam::driver::{DriverError, Released};

use super::{go, mac, printing, quiet, rig, root_row, stdout_at, Rig};
use crate::ops::common::release::testkit::framed;
use crate::store::lease::{retry_after, LeaseRow};

/// A Box outside the Mac layout whose one teardown step fails.
fn failing_plain() -> Rig {
    let driver = printing(&[framed(0, "compose stuck")]).exec_results([super::exit(1)].map(Ok));
    rig(driver, "/work/box-1")
}

fn rows(r: &Rig) -> Vec<LeaseRow> {
    r.cx.store().unwrap().leases(r.id).unwrap()
}

#[test]
fn a_driver_release_failure_is_recorded_on_every_row_with_a_backoff() {
    let r = mac(quiet().fail_release(DriverError::Failed {
        summary: "gone wrong".into(),
    }));
    let now = r.cx.clock().now();
    let report = go(&r);
    assert!(report.driver.is_err());
    assert!(!report.released);
    let rows = rows(&r);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.attempts, 1);
        assert!(row
            .failure
            .as_deref()
            .unwrap()
            .contains("Box release failed"));
        assert!(row.failure.as_deref().unwrap().contains("gone wrong"));
        assert_eq!(
            row.next_attempt_at,
            Some(now.saturating_add(retry_after(1)))
        );
    }
    assert_eq!(retry_after(1), Duration::from_mins(10));
}

#[test]
fn a_driver_failure_joins_the_step_failures_in_the_same_attempt() {
    let driver = printing(&stdout_at(0, "agent stuck"))
        .exec_results([super::exit(1)].map(Ok))
        .fail_release(DriverError::Failed {
            summary: "gone wrong".into(),
        });
    let r = mac(driver);
    go(&r);
    let row = root_row(&r);
    assert_eq!(row.attempts, 1);
    let failure = row.failure.unwrap();
    assert!(failure.contains("stop the agent: agent stuck"), "{failure}");
    assert!(failure.contains("Box release failed"), "{failure}");
}

#[test]
fn a_deleted_box_with_no_mac_layout_drops_the_rows_of_a_failed_step() {
    let r = failing_plain();
    let report = go(&r);
    assert_eq!(report.driver, Ok(Released::Deleted));
    assert!(report.released);
    assert!(rows(&r).is_empty());
    assert_eq!(report.left.len(), 1);
}

#[test]
fn an_already_gone_box_with_no_mac_layout_drops_the_rows_of_a_failed_step() {
    let r = failing_plain();
    r.driver.forget(&r.handle);
    let report = go(&r);
    assert_eq!(report.driver, Ok(Released::AlreadyGone));
    assert!(report.released);
    assert!(rows(&r).is_empty());
}

#[test]
fn a_deleted_mac_box_keeps_the_rows_of_a_failed_step() {
    let driver = printing(&stdout_at(0, "agent stuck")).exec_results([super::exit(1)].map(Ok));
    let r = mac(driver);
    let report = go(&r);
    assert_eq!(report.driver, Ok(Released::Deleted));
    assert!(!report.released);
    let row = root_row(&r);
    assert_eq!(row.attempts, 1);
    assert!(row.failure.as_deref().unwrap().contains("agent stuck"));
    assert!(row.next_attempt_at.is_some());
}

#[test]
fn a_deleted_box_on_a_static_host_keeps_the_rows_of_a_failed_step() {
    let driver = printing(&[framed(0, "compose stuck")])
        .exec_results([super::exit(1)].map(Ok))
        .static_host();
    let r = rig(driver, "/work/box-1");
    let report = go(&r);
    assert_eq!(report.driver, Ok(Released::Deleted));
    assert!(!report.released);
    let failures: Vec<_> = rows(&r).into_iter().filter_map(|l| l.failure).collect();
    assert!(
        failures.iter().any(|f| f.contains("compose stuck")),
        "{failures:?}"
    );
}
