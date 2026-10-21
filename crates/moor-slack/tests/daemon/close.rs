//! Closing a Box through the coordinator's `close_box`: the fake `claude` calls the front's
//! real tool server in a turn that holds the requester's message, the worker sends the close
//! without force, posts the result or the controller's refusal, asks again after a lost
//! connection, and finishes a close an earlier run left. A reply reading `close` is text for
//! the coordinator.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::{Path, PathBuf};

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::inbox::reply;
use super::support::controller::{failed, FakeController, Scripted};
use super::support::{Rig, BOX_NAME, QUIET, TS, USER};

/// A token the fake answers with its tool calls for.
const MCP: &str = "sk-ant-oat01-mcp-c105e";

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

/// A coordinated daemon whose thread [`TS`] runs Box [`BOX_NAME`], started by the start
/// command, with the fake `claude` set to call `close_box` on it in every turn.
fn running_box() -> (Rig, FakeController, PathBuf) {
    let (rig, controller, bin) = Rig::start_coordinated(Some(MCP));
    controller.keep_boxes();
    Rig::tool_calls(&bin, None, &[("close_box", json!({"box": BOX_NAME}))]);
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.wait_post(&format!("Started Box `{BOX_NAME}`."));
    rig.wait_box_state(BOX_NAME, "bound");
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
    let said = format!("Box `{BOX_NAME}` is closing. The thread will say when it has closed.");
    assert_eq!(results(&bin, 0), [(said, false)]);
    assert_eq!(
        closes_and_shows(&controller),
        [("close", BOX_NAME.to_owned(), false)]
    );
    Rig::tool_calls(&bin, None, &[]);
    reply(&rig, 3, USER, "thanks");
    rig.slack.wait_updates(2);
    let stdin = std::fs::read_to_string(bin.join("run-1/stdin")).unwrap();
    let journal = format!(
        "## Actions this thread took\nTurn 1, <@{USER}> asked. close_box Box `{BOX_NAME}`. \
         Result: Box `{BOX_NAME}` is closing.\n"
    );
    assert!(stdin.contains(&journal), "{stdin}");
    assert!(
        stdin.contains(&format!(
            "- Box `{BOX_NAME}` on demo, asked for by <@{USER}>: closed"
        )),
        "{stdin}"
    );
}

#[test]
fn close_refused_with_unpushed_work_is_posted_and_the_box_stays_open() {
    let (rig, controller, _bin) = running_box();
    controller.script("close", Scripted::Reply(failed("close", UNPUSHED)));
    reply(&rig, 2, USER, "close it");
    rig.wait_post(&format!("Could not close Box `{BOX_NAME}`: {UNPUSHED}"));
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
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    rig.link_token(USER, "sk-ant-oat01-good-c105e");
    let controller = rig.serve(PROTOCOL);
    controller.keep_boxes();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.wait_post(&format!("Started Box `{BOX_NAME}`."));
    reply(&rig, 2, USER, "close");
    rig.slack.wait_updates(1);
    let stdin = std::fs::read_to_string(bin.join("run-0/stdin")).unwrap();
    assert!(
        stdin.ends_with(&format!("## New\n[<@{USER}>] close\n")),
        "{stdin}"
    );
    std::thread::sleep(QUIET);
    assert_eq!(closes_and_shows(&controller), []);
}
