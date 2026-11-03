//! Tests for deleting setup checkpoints when the disk they live on falls below its floor.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::decide::schedule::DISK_FLOOR_MIB;
use moor_core::{BoxName, CloseReq};

use super::warm::{after_a_partial_setup, events, start, LINES};
use super::World;
use crate::ops::close;
use crate::testing::driver::FakeDriver;

/// A disk whose floor is [`DISK_FLOOR_MIB`], plenty free, each checkpoint taking 10 MiB.
fn roomy() -> FakeDriver {
    FakeDriver::new().with_checkpoint_disk(100_000, 90_000, 10)
}

fn close(w: &World, name: &str) {
    let req = CloseReq {
        target: BoxName::parse_allow_bare(name).unwrap().into(),
        force: true,
    };
    close::run(&w.cx, &req).unwrap();
}

fn deleted(w: &World) -> Vec<String> {
    let calls = w.driver.calls();
    let gone = calls
        .iter()
        .filter_map(|c| c.strip_prefix("delete-checkpoint "));
    gone.map(str::to_owned).collect()
}

#[test]
fn above_the_floor_no_checkpoint_is_deleted() {
    let (w, _) = after_a_partial_setup(roomy());
    start(&w, "second", "def456", LINES);
    assert!(deleted(&w).is_empty(), "{:?}", w.driver.calls());
}

#[test]
fn below_the_floor_a_start_deletes_the_least_recently_used_and_says_so() {
    let (w, _) = after_a_partial_setup(roomy());
    assert_eq!(w.driver.checkpoint_ids(), ["ckpt-1", "ckpt-2"]);
    close(&w, "first");
    w.driver.set_free_mib(DISK_FLOOR_MIB - 5);
    let second = start(&w, "second", "def456", LINES);
    assert_eq!(deleted(&w)[0], "ckpt-1", "the clone was used longest ago");
    let told = events(&w, second, "checkpoint_evicted");
    let first_told = told.iter().find(|e| e["checkpoint"] == "ckpt-1").unwrap();
    assert_eq!(first_told["checkpoint"], "ckpt-1");
    assert_eq!(first_told["reason"], "least_recently_used");
    assert_eq!(first_told["bytes"], 10 * 1024 * 1024);
    assert_eq!(first_told["needs_mib"], DISK_FLOOR_MIB);
}

#[test]
fn a_newer_commit_s_chain_sends_the_older_one_first() {
    let (w, _) = after_a_partial_setup(roomy());
    close(&w, "first");
    start(&w, "newer", "def456", LINES);
    close(&w, "newer");
    start(&w, "again", "abc123", LINES);
    assert!(w.driver.calls().contains(&"fork ckpt-2".to_owned()));
    close(&w, "again");
    w.driver.set_free_mib(DISK_FLOOR_MIB - 15);
    let third = start(&w, "third", "fed789", LINES);
    assert_eq!(
        deleted(&w)[..2],
        ["ckpt-1", "ckpt-2"],
        "the older commit's chain goes first though a Box used it last"
    );
    let told = events(&w, third, "checkpoint_evicted");
    assert!(told.iter().any(|e| e["reason"] == "superseded"), "{told:?}");
}

#[test]
fn a_box_whose_setup_failed_keeps_the_checkpoint_its_retry_resumes_from() {
    let (w, _) = after_a_partial_setup(roomy());
    w.driver.set_free_mib(DISK_FLOOR_MIB - 15);
    start(&w, "second", "def456", LINES);
    let gone = deleted(&w);
    assert!(gone.contains(&"ckpt-1".to_owned()), "{gone:?}");
    assert!(!gone.contains(&"ckpt-2".to_owned()), "{gone:?}");
    assert!(w.driver.checkpoint_ids().contains(&"ckpt-2".to_owned()));
    let warned = w.cx.take_warnings();
    assert!(
        warned.iter().any(|x| x.code == "checkpoint_skipped"),
        "with nothing left to delete, no new checkpoint fills the disk: {warned:?}"
    );
}

#[test]
fn a_box_forking_a_checkpoint_keeps_it() {
    let (w, _) = after_a_partial_setup(roomy());
    close(&w, "first");
    w.driver.set_free_mib(DISK_FLOOR_MIB - 15);
    start(&w, "second", "abc123", LINES);
    assert!(w.driver.calls().contains(&"fork ckpt-2".to_owned()));
    assert_eq!(deleted(&w), ["ckpt-1"]);
}

#[test]
fn a_checkpoint_whose_delete_fails_stays_tracked_and_goes_later() {
    let (w, _) = after_a_partial_setup(roomy());
    close(&w, "first");
    w.driver.set_free_mib(DISK_FLOOR_MIB - 15);
    w.driver.set_deletes_fail(true);
    let second = start(&w, "second", "def456", LINES);
    let tried = deleted(&w);
    let mut once = tried.clone();
    once.dedup();
    assert_eq!(tried, once, "each checkpoint is tried once per start");
    assert!(tried.contains(&"ckpt-1".to_owned()), "{tried:?}");
    assert!(events(&w, second, "checkpoint_evicted").is_empty());
    let warned = w.cx.take_warnings();
    assert!(
        warned.iter().any(|x| x.code == "checkpoint_delete_failed"),
        "{warned:?}"
    );
    close(&w, "second");
    w.driver.set_deletes_fail(false);
    w.driver.set_free_mib(DISK_FLOOR_MIB - 15);
    let third = start(&w, "third", "fed789", LINES);
    let told = events(&w, third, "checkpoint_evicted");
    assert!(
        told.iter().any(|e| e["checkpoint"] == "ckpt-1"),
        "the checkpoint left on the disk is still tracked and goes once it can: {told:?}"
    );
}
