//! Closing a Box through the coordinator's `close_box`: the fake `claude` calls the front's
//! real tool server in a turn that holds the requester's message, the worker sends the close
//! without force, posts the result or the controller's refusal, asks again after a lost
//! connection, and finishes a close an earlier run left. A reply reading `close` is text for
//! the coordinator.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::{Path, PathBuf};

use moor_core::decide::close::Refusal;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::inbox::reply;
use super::support::controller::{failed, FakeController, Scripted};
use super::support::{Rig, BOX_NAME, QUIET, USER};

/// The controller's refusal of a close that would lose unpushed work.
/// The controller's refusal of a close of the Box, which holds 2 unpushed commits.
fn unpushed() -> String {
    Refusal::UnsavedInBox {
        name: BOX_NAME.into(),
        branch: format!("moor/{BOX_NAME}"),
        ahead: 2,
        dirty: 0,
    }
    .to_string()
}

/// A `close` reply for the Box.
fn closed() -> Scripted {
    Scripted::Reply(Response::ok(
        "close",
        Some(json!({"box": {"id": 1, "name": BOX_NAME, "lifecycle": "closed"}})),
    ))
}

/// A coordinated daemon whose thread runs Box [`BOX_NAME`], started by `start_box`, with the
/// fake `claude` set to call `close_box` on it in every later turn.
fn running_box() -> (Rig, FakeController, PathBuf) {
    let (rig, controller, bin) = Rig::start_boxes();
    Rig::tool_calls(&bin, None, &[("close_box", json!({"box": BOX_NAME}))]);
    rig.open_thread_with_a_box();
    (rig, controller, bin)
}

/// The command, Box name and force flag of every `close` and `show` the controller got.
fn closes_and_shows(controller: &FakeController) -> Vec<(&'static str, String, bool)> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Close(c) => Some(("close", c.target.to_string(), c.force)),
                Request::Show(s) if !s.agent => {
                    Some(("show", s.target.as_ref()?.to_string(), false))
                }
                _ => None,
            })
            .collect()
    })
}

/// The tool results of run `run` once it has them.
fn results(bin: &Path, run: u32) -> Vec<(String, bool)> {
    for _ in 0..500 {
        if bin.join(format!("run-{run}/mcp")).exists() {
            let got = Rig::tool_results(bin, run);
            if !got.is_empty() {
                return got;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("run {run} called no tool");
}

#[test]
fn close_box_in_the_requesters_turn_closes_the_box_without_force() {
    let (rig, controller, bin) = running_box();
    controller.script("close", closed());
    reply(&rig, 2, USER, "we are done, close it");
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    let said = format!(
        "Asked to close Box `{BOX_NAME}`. It is not closed yet. The thread will be told whether \
         it closed or was refused, and a `[moor]` line in your next turn says which."
    );
    assert_eq!(results(&bin, 1), [(said, false)]);
    assert_eq!(
        closes_and_shows(&controller),
        [("close", BOX_NAME.to_owned(), false)]
    );
    Rig::tool_calls(&bin, None, &[]);
    reply(&rig, 3, USER, "thanks");
    rig.wait_replies(3);
    let stdin = std::fs::read_to_string(bin.join("run-2/stdin")).unwrap();
    let journal = format!(
        "Turn 2, <@{USER}> asked. close_box Box `{BOX_NAME}`. Result: Asked to close Box \
         `{BOX_NAME}`.\n"
    );
    assert!(stdin.contains(&journal), "{stdin}");
    assert!(
        stdin.contains(&format!(
            "- Box `{BOX_NAME}` on acme/demo, asked for by <@{USER}>: closed"
        )),
        "{stdin}"
    );
}

#[test]
fn close_refused_with_unpushed_work_is_posted_and_the_box_stays_open() {
    let (rig, controller, _bin) = running_box();
    controller.script("close", Scripted::Reply(failed("close", &unpushed())));
    reply(&rig, 2, USER, "close it");
    rig.wait_post(&format!(
        "Box `{BOX_NAME}` is still open: it holds work that is not saved anywhere (2 unpushed \
         commits). Closing it would discard that work, so nothing is lost unless you confirm. \
         If you still want it closed, say so in this thread."
    ));
    controller.script("close", closed());
    reply(&rig, 3, USER, "I pushed, close it");
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    assert_eq!(
        closes_and_shows(&controller).len(),
        2,
        "a second close was sent"
    );
}

#[test]
fn close_lost_with_its_connection_is_asked_about_and_sent_again() {
    let (rig, controller, _bin) = running_box();
    controller.script("close", Scripted::Hangup);
    controller.script("close", closed());
    reply(&rig, 2, USER, "close it");
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    let want = [
        ("close", BOX_NAME.to_owned(), false),
        ("show", BOX_NAME.to_owned(), false),
        ("close", BOX_NAME.to_owned(), false),
    ];
    assert_eq!(closes_and_shows(&controller), want);
    std::thread::sleep(QUIET);
    let posted = rig.slack.posts();
    let closed = posted.iter().filter(|p| p.text.starts_with("Closed Box"));
    assert_eq!(closed.count(), 1, "the close is posted once");
}

#[test]
fn a_box_left_closing_by_a_stopped_daemon_is_closed_at_startup() {
    let (mut rig, controller, _bin) = running_box();
    controller.script("close", Scripted::Hold);
    reply(&rig, 2, USER, "close it");
    controller.wait_until("the held close", |seen| {
        seen.requests.iter().any(|r| matches!(r, Request::Close(_)))
    });
    rig.kill_daemon();
    controller.script("close", closed());
    rig.spawn_daemon();
    rig.wait_post(&format!("Closed Box `{BOX_NAME}`."));
    let want = [
        ("close", BOX_NAME.to_owned(), false),
        ("show", BOX_NAME.to_owned(), false),
        ("close", BOX_NAME.to_owned(), false),
    ];
    assert_eq!(closes_and_shows(&controller), want);
}

#[test]
fn a_reply_reading_close_reaches_the_coordinator_as_text_and_closes_nothing() {
    let (rig, controller, bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    reply(&rig, 2, USER, "close");
    rig.wait_replies(2);
    let stdin = std::fs::read_to_string(bin.join("run-1/stdin")).unwrap();
    assert!(
        stdin.ends_with(&format!(
            "## New\n[moor] Box `{BOX_NAME}` started.\n[<@{USER}>] close\n"
        )),
        "{stdin}"
    );
    std::thread::sleep(QUIET);
    assert_eq!(closes_and_shows(&controller), []);
}
