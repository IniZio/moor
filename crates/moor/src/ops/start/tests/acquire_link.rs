//! A machine's link that drops while it opens a Box: the start waits for the relink and asks for
//! the same Box again, and an open the machine refuses still fails at once.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::DriverError;

use super::interrupted::{box_row, releases};
use super::setup::start_req;
use super::{script_repo, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

/// What the machine adapter says when the link drops under an open's stream.
fn stream_ended() -> DriverError {
    DriverError::Unavailable("the stream ended: unexpected end of file".to_owned())
}

/// What the machine adapter says while the controller holds no link to the machine.
fn unlinked() -> DriverError {
    DriverError::Unavailable("machine 1 is not linked".to_owned())
}

/// A world whose machine's first open fails with `cut`, then its next `times` asks with `again`.
fn opened(cut: DriverError, (times, again): (usize, DriverError)) -> World {
    let driver = FakeDriver::new().cut_acquires_go_on().fail_acquire(cut);
    let w = world(driver.adopt_fails(times, &again));
    script_repo(&w, None);
    w
}

fn kinds(w: &World) -> Vec<String> {
    let id = box_row(w).id;
    let events = w.cx.store().unwrap().events(id, 200).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn count(calls: &[String], call: &str) -> usize {
    calls.iter().filter(|c| *c == call).count()
}

#[test]
fn a_link_drop_mid_acquire_waits_for_the_relink_and_the_box_comes_up() {
    let w = opened(stream_ended(), (30, unlinked()));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::Ready);
    let kinds = kinds(&w);
    let reattached = kinds.iter().filter(|k| *k == "acquire_reattached").count();
    assert_eq!(reattached, 1, "{kinds:?}");
    assert!(!kinds.iter().any(|k| k == "setup_failed"), "{kinds:?}");
    let calls = w.driver.calls();
    assert_eq!(
        count(&calls, "acquire"),
        2,
        "one open, one that found it: {calls:?}"
    );
    assert_eq!(count(&calls, "adopt"), 31, "{calls:?}");
    assert!(releases(&w).is_empty());
}

#[test]
fn a_refused_open_fails_at_once_without_asking_again() {
    let refused = DriverError::Failed {
        summary: "pulling ghcr.io/inizio/moor-work:nope failed: manifest unknown".to_owned(),
    };
    let w = opened(refused, (0, unlinked()));
    assert!(run(&w.cx, &start_req()).is_err());
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SetupFailed);
    let kinds = kinds(&w);
    assert!(
        !kinds.iter().any(|k| k == "acquire_reattached"),
        "{kinds:?}"
    );
    assert_eq!(count(&w.driver.calls(), "adopt"), 0);
}

#[test]
fn a_link_down_past_the_wait_fails_the_start_with_what_the_link_said() {
    let w = opened(stream_ended(), (300, unlinked()));
    assert!(run(&w.cx, &start_req()).is_err());
    assert_eq!(box_row(&w).lifecycle, BoxLifecycle::SetupFailed);
    let id = box_row(&w).id;
    let events = w.cx.store().unwrap().events(id, 400).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert!(
        failed.payload.to_string().contains("not linked"),
        "{:?}",
        failed.payload
    );
}
