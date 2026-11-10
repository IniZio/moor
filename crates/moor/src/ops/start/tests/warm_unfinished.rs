//! A kept checkpoint the provider never finished: a new Box starts from the layer below it, and
//! it is not offered again.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::seam::driver::DriverError;

use super::warm::{after_a_partial_setup, events, lines_on, start, LINES};
use crate::testing::driver::FakeDriver;

fn unfinished() -> DriverError {
    DriverError::UnfinishedCheckpoint {
        summary: "checkpoint ckpt-2 was never finished; delete it".to_owned(),
    }
}

#[test]
fn an_unfinished_top_checkpoint_is_skipped_for_the_layer_below_and_never_offered_again() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().fork_fails_once(unfinished()));
    let second = start(&w, "second", "abc123", LINES);
    assert!(events(&w, second, "warm_start_failed").is_empty());
    let warm = events(&w, second, "warm_started");
    assert_eq!(warm.len(), 1, "{warm:?}");
    assert_eq!(
        warm[0]["layer"], 0,
        "the clone's layer, below the unfinished one"
    );
    assert!(
        w.driver.calls().contains(&"fork ckpt-1".to_owned()),
        "{:?}",
        w.driver.calls()
    );
    assert_eq!(lines_on(&w, second), ["one-a", "two-b", "three-c"]);
    let third = start(&w, "third", "abc123", LINES);
    assert_eq!(events(&w, third, "warm_started").len(), 1);
    let calls = w.driver.calls();
    let dead = calls.iter().filter(|c| *c == "fork ckpt-2").count();
    assert_eq!(dead, 1, "{calls:?}");
    assert!(events(&w, third, "warm_start_failed").is_empty());
}
