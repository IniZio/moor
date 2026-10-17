//! Tests for the lease-path layout.

use moor_core::seam::clock::Ts;
use moor_core::task_key::TaskKey;
use moor_core::{TaskId, TaskName};

use super::layout_from_leases;
use crate::store::lease::{LeaseId, LeaseKind, LeaseRow, LeaseState};

/// The root a lease-derived layout gives for `key` when the Task holds one row per `(kind, path)`.
fn lease_root(key: &str, leases: &[(LeaseKind, &str)]) -> Option<String> {
    let rows: Vec<LeaseRow> = leases
        .iter()
        .zip(1..)
        .map(|((kind, path), id)| LeaseRow {
            id: LeaseId(id),
            task: TaskId::new(1),
            kind: *kind,
            key: (*path).to_owned(),
            state: LeaseState::Held,
            booted: false,
            process: None,
            size_at_release: None,
            updated_at: Ts::from_millis(0),
            attempts: 0,
            failure: None,
            next_attempt_at: None,
        })
        .collect();
    layout_from_leases(&rows, &TaskKey::new(&TaskName::parse(key).unwrap()))
        .map(|l| l.root.as_str().to_owned())
}

#[test]
fn a_work_root_holding_the_key_component_keeps_the_last_key_as_the_root() {
    let got = lease_root("moor", &[(LeaseKind::BoxDir, "/Users/box/moor/moor/work")]);
    assert_eq!(got.as_deref(), Some("/Users/box/moor/moor"));
}

#[test]
fn a_cache_path_below_home_that_repeats_the_key_keeps_the_real_root() {
    let got = lease_root(
        "envchk",
        &[(LeaseKind::Home, "/w/envchk/home/.cache/envchk")],
    );
    assert_eq!(got.as_deref(), Some("/w/envchk"));
}

#[test]
fn a_key_that_is_only_the_start_of_a_component_gives_no_layout() {
    let got = lease_root("app", &[(LeaseKind::BoxDir, "/w/app-two/work")]);
    assert_eq!(got, None);
}

#[test]
fn a_key_followed_by_a_non_layout_directory_gives_no_layout() {
    let got = lease_root("app", &[(LeaseKind::BoxDir, "/w/app/other/work")]);
    assert_eq!(got, None);
}

#[test]
fn rows_that_name_different_roots_give_no_layout() {
    let got = lease_root(
        "demo",
        &[
            (LeaseKind::BoxDir, "/a/demo/work"),
            (LeaseKind::BoxRoot, "/b/demo/home"),
        ],
    );
    assert_eq!(got, None);
}

#[test]
fn rows_that_name_the_same_root_give_that_layout() {
    let got = lease_root(
        "demo",
        &[
            (LeaseKind::BoxDir, "/a/demo/work/repo"),
            (LeaseKind::BoxRoot, "/a/demo/home"),
        ],
    );
    assert_eq!(got.as_deref(), Some("/a/demo"));
}

#[test]
fn a_lease_path_equal_to_the_root_gives_no_layout() {
    let got = lease_root("demo", &[(LeaseKind::BoxRoot, "/w/demo")]);
    assert_eq!(got, None);
}

#[test]
fn a_sibling_that_only_starts_with_a_layout_directory_name_gives_no_layout() {
    let got = lease_root("demo", &[(LeaseKind::BoxDir, "/w/demo/homework/x")]);
    assert_eq!(got, None);
}
