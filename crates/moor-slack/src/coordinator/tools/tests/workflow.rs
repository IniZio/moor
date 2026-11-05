//! Tests for the workflow tools: the starter gate and the request each sends.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{turn_of, Controller};
use crate::coordinator::tools::{names, Call, Reply};
use crate::db::{Db, SlackThread};

fn thread(dir: &std::path::Path) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    db
}

fn add() -> Call {
    Call::parse(
        "add_workflow",
        &json!({"name": "sec", "cron": "0 9 * * *", "timezone": "Asia/Hong_Kong",
            "repos": ["acme/api", "acme/web"], "prompt": "fix security alerts"}),
    )
    .unwrap()
}

fn go(c: &Controller, db: &Db, call: &Call) -> Reply {
    c.run_with(db, call, &mut |r| {
        Ok(Response::ok(
            r.cmd(),
            Some(json!({"name": "sec", "changed": true})),
        ))
    })
}

#[test]
fn adding_a_workflow_names_this_thread_as_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    let db = thread(dir.path());
    turn_of(&db, "U1", "m1");
    let c = Controller::new();
    let reply = go(&c, &db, &add());
    assert!(!reply.is_error, "{}", reply.text);
    let sent = c.sent.borrow();
    let Request::WorkflowAdd(req) = &sent[0] else {
        panic!("{:?}", sent[0])
    };
    assert_eq!(req.origin_key.as_deref(), Some("C1/1.0"));
    assert_eq!((req.identity.is_none(), req.repos.len()), (true, 2));
    assert_eq!(
        db.workflow_owners().unwrap(),
        ["U1"],
        "the reporter follows the starter now"
    );
}

#[test]
fn every_workflow_tool_needs_the_starter_to_have_written_in_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db = thread(dir.path());
    turn_of(&db, "U2", "m1");
    let c = Controller::new();
    for call in [
        add(),
        Call::ListWorkflows,
        Call::RemoveWorkflow { name: "sec".into() },
        Call::SetWorkflowEnabled {
            name: "sec".into(),
            enabled: false,
        },
        Call::ListWorkflowRuns {
            after: None,
            limit: None,
        },
    ] {
        let reply = go(&c, &db, &call);
        assert!(
            reply.is_error && reply.text.contains("<@U1>"),
            "{call:?}: {}",
            reply.text
        );
    }
    assert!(c.sent.borrow().is_empty());
}

#[test]
fn the_other_workflow_tools_send_their_requests() {
    let dir = tempfile::tempdir().unwrap();
    let db = thread(dir.path());
    turn_of(&db, "U1", "m1");
    let c = Controller::new();
    for call in [
        Call::ListWorkflows,
        Call::RemoveWorkflow { name: "sec".into() },
        Call::SetWorkflowEnabled {
            name: "sec".into(),
            enabled: false,
        },
        Call::ListWorkflowRuns {
            after: Some(4),
            limit: None,
        },
    ] {
        assert!(!go(&c, &db, &call).is_error);
    }
    let cmds: Vec<_> = c.sent.borrow().iter().map(Request::cmd).collect();
    assert_eq!(
        cmds,
        [
            "workflow-list",
            "workflow-remove",
            "workflow-set-enabled",
            "workflow-runs"
        ]
    );
    assert!(names().contains(&"add_workflow"));
    assert!(
        db.workflow_owners().unwrap().is_empty(),
        "only an add starts the reports"
    );
}
