//! `close` in a thread: the starter closes the thread's Box with no coordinator, the
//! controller's refusal is posted, and a close lost with its connection is asked about again.

#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::inbox::{inbox, reply, started_thread};
use super::support::controller::{failed, FakeController, Scripted};
use super::support::{BOX_NAME, TS, USER};

/// The controller's refusal of a close that would lose unpushed work.
const UNPUSHED: &str = "the host worktree has work that is not saved (2 unpushed commit(s), \
     no uncommitted changes); push it, or pass --force to close anyway";

/// A `close` reply for the Box.
fn closed() -> Scripted {
    Scripted::Reply(Response::ok(
        "close",
        Some(json!({"box": {"id": 1, "name": BOX_NAME, "lifecycle": "closed"}})),
    ))
}

/// The command, Box name and force flag of every request after the start.
fn after_start(controller: &FakeController) -> Vec<(&'static str, String, bool)> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Close(c) => Some(("close", c.target.to_string(), c.force)),
                Request::Show(s) => Some(("show", s.target.as_ref()?.to_string(), false)),
                _ => None,
            })
            .collect()
    })
}

#[test]
fn close_from_the_starter_closes_the_box_with_no_coordinator() {
    let (rig, controller) = started_thread();
    controller.script("close", closed());
    reply(&rig, 2, USER, "close");
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].thread_ts, TS);
    assert_eq!(posts[1].text, format!("Closed Box `{BOX_NAME}`."));
    assert_eq!(
        after_start(&controller),
        [("close", BOX_NAME.to_owned(), false)]
    );
    let item = (
        "close".to_owned(),
        USER.to_owned(),
        "close".to_owned(),
        "Ev2".to_owned(),
    );
    assert_eq!(inbox(&rig), [item]);
}

#[test]
fn close_refused_with_unpushed_work_is_posted_and_the_box_stays_open() {
    let (rig, controller) = started_thread();
    controller.script("close", Scripted::Reply(failed("close", UNPUSHED)));
    reply(&rig, 2, USER, "close");
    let posts = rig.slack.wait_posts(2);
    assert_eq!(
        posts[1].text,
        format!("Could not close Box `{BOX_NAME}`: {UNPUSHED}")
    );
    controller.script("close", closed());
    reply(&rig, 3, USER, "close");
    let posts = rig.slack.wait_posts(3);
    assert_eq!(posts[2].text, format!("Closed Box `{BOX_NAME}`."));
    assert_eq!(after_start(&controller).len(), 2, "a second close was sent");
}

#[test]
fn close_lost_with_its_connection_is_asked_about_and_sent_again() {
    let (rig, controller) = started_thread();
    controller.script("close", Scripted::Hangup);
    controller.script("close", closed());
    reply(&rig, 2, USER, "close");
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].text, format!("Closed Box `{BOX_NAME}`."));
    let want = [
        ("close", BOX_NAME.to_owned(), false),
        ("show", BOX_NAME.to_owned(), false),
        ("close", BOX_NAME.to_owned(), false),
    ];
    assert_eq!(after_start(&controller), want);
    std::thread::sleep(super::support::QUIET);
    assert_eq!(rig.slack.posts().len(), 2, "the close is posted once");
}

#[test]
fn a_box_left_closing_by_a_stopped_daemon_is_closed_at_startup() {
    let (mut rig, controller) = started_thread();
    controller.script("close", Scripted::Hold);
    reply(&rig, 2, USER, "close");
    controller.wait_until("the held close", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Close(_)))
    });
    rig.kill_daemon();
    controller.script("close", closed());
    rig.spawn_daemon();
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].text, format!("Closed Box `{BOX_NAME}`."));
    let want = [
        ("close", BOX_NAME.to_owned(), false),
        ("show", BOX_NAME.to_owned(), false),
        ("close", BOX_NAME.to_owned(), false),
    ];
    assert_eq!(after_start(&controller), want);
}
