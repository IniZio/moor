//! Tests for the coordinator's tools against a real `slack.db` and a scripted controller.
#![allow(clippy::panic, reason = "test code")]

use std::path::Path;

use moor_core::name::{BoxId, BoxName};
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{run, Call, Reply, Scope};
use crate::db::{BoxState, Db, SlackBox, SlackThread};

/// A database whose thread `1.0` of channel `C1` holds Box `one`, asked for by `U1` and
/// running, and Box `two`, asked for by `U2` and still starting.
fn two_boxes(dir: &Path) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        repo: "acme/api".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    for (name, by) in [("slack-1-0-1", "U1"), ("slack-1-0-2", "U2")] {
        let boxed = SlackBox {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            box_name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "acme/api".into(),
            goal: format!("goal of {name}"),
            requested_by: by.into(),
            state: BoxState::Starting,
        };
        db.insert_starting(&boxed, 1).unwrap();
    }
    let one = BoxName::parse_allow_bare("slack-1-0-1").unwrap();
    db.bind(&one, BoxId::new(1), Some("moor/b")).unwrap();
    db
}

/// Runs `call` in thread `1.0`, answering each request with `answer`; returns the reply and
/// what the controller was sent.
fn run_with(db: &Db, call: &Call, answer: &dyn Fn(&Request) -> Response) -> (Reply, Vec<Request>) {
    let mut requests = Vec::new();
    let mut send = |request: Request| {
        let reply = answer(&request);
        requests.push(request);
        Ok(reply)
    };
    let mut scope = Scope {
        db,
        channel: "C1",
        thread_ts: "1.0",
        call: &mut send,
    };
    let reply = run(&mut scope, call);
    (reply, requests)
}

#[test]
fn list_boxes_shows_each_box_with_the_controllers_view_of_a_running_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let view =
        json!({"box": {"lifecycle": "running", "branch": "moor/b", "agent": {"state": "working"}}});
    let (reply, sent) = run_with(&db, &Call::ListBoxes, &|r| {
        Response::ok(r.cmd(), Some(view.clone()))
    });
    let want = "- Box `slack-1-0-1` on acme/api, asked for by <@U1>: lifecycle running, agent \
                working, branch moor/b\n- Box `slack-1-0-2` on acme/api, asked for by <@U2>: \
                starting";
    assert_eq!(reply, Reply::done(want));
    let asked: Vec<_> = sent
        .iter()
        .map(|r| match r {
            Request::Show(s) => (s.target.as_ref().unwrap().to_string(), s.agent),
            other => panic!("only show is sent: {other:?}"),
        })
        .collect();
    assert_eq!(
        asked,
        [("slack-1-0-1".to_owned(), true)],
        "a starting Box is not asked about"
    );
}

#[test]
fn list_boxes_says_when_the_controller_did_not_answer() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let mut send = |_: Request| {
        Err(moor_client::ClientError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )))
    };
    let mut scope = Scope {
        db: &db,
        channel: "C1",
        thread_ts: "1.0",
        call: &mut send,
    };
    let reply = run(&mut scope, &Call::ListBoxes);
    assert!(
        reply.text.starts_with(
            "- Box `slack-1-0-1` on acme/api, asked for by <@U1>: the controller is not answering"
        ),
        "{reply:?}"
    );
}

#[test]
fn list_boxes_in_a_thread_with_no_box_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let mut send =
        |_: Request| -> Result<Response, moor_client::ClientError> { panic!("nothing is sent") };
    let mut scope = Scope {
        db: &db,
        channel: "C1",
        thread_ts: "9.9",
        call: &mut send,
    };
    assert_eq!(
        run(&mut scope, &Call::ListBoxes),
        Reply::done("This thread has no Box.")
    );
}

#[test]
fn an_unknown_tool_is_refused() {
    assert_eq!(
        Call::parse("rm_rf", &json!({})),
        Err("There is no tool `rm_rf`.".to_owned())
    );
}
