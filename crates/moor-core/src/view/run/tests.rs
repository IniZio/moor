//! Tests for the run, wait and cp payload shapes.

use serde_json::json;

use super::{CpValue, RunValue, WaitRunView, WaitValue};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::run::SyncMode;
use crate::domain::session_state::SessionState;
use crate::name::RunId;
use crate::request::WaitFor;
use crate::view::TaskView;

fn task() -> TaskView {
    TaskView {
        id: 1,
        name: "t".into(),
        repo: "r".into(),
        issue: None,
        goal: None,
        lifecycle: BoxLifecycle::Ready,
        session_state: SessionState::Working,
        state_reason: None,
        base: None,
        branch: None,
        worktree: None,
        workspace: None,
        r#box: None,
        agent: None,
        run: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
        attention: Vec::new(),
    }
}

#[test]
fn run_value_shape() {
    let v = RunValue {
        ran: true,
        run: Some(RunId::new(7)),
        exit: Some(0),
        interrupted: false,
        sha: Some("abcd".into()),
        sync: SyncMode::Full,
        record: None,
        log: "/l".into(),
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        json!({"ran": true, "run": 7, "exit": 0, "interrupted": false, "sha": "abcd",
               "sync": "full", "record": null, "log": "/l"})
    );
}

#[test]
fn refused_run_value_has_null_run_and_exit() {
    let v = RunValue {
        ran: false,
        run: None,
        exit: None,
        interrupted: false,
        sha: Some("abcd".into()),
        sync: SyncMode::None,
        record: Some("rec".into()),
        log: "/l".into(),
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        json!({"ran": false, "run": null, "exit": null, "interrupted": false, "sha": "abcd",
               "sync": "none", "record": "rec", "log": "/l"})
    );
}

#[test]
fn wait_value_shape() {
    let run = WaitRunView {
        id: RunId::new(3),
        argv: vec!["make".into()],
        in_flight: true,
        exit: None,
        interrupted: false,
    };
    let task_json = serde_json::to_value(task()).unwrap();
    let v = WaitValue {
        task: task(),
        timed_out: false,
        waited_ms: 5,
        wait_for: WaitFor::Run,
        run: Some(run),
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        json!({"task": task_json, "timed_out": false, "waited_ms": 5, "for": "run",
               "run": {"id": 3, "argv": ["make"], "in_flight": true, "exit": null,
                       "interrupted": false}})
    );
}

#[test]
fn wait_value_without_run_is_null() {
    let task_json = serde_json::to_value(task()).unwrap();
    let v = WaitValue {
        task: task(),
        timed_out: true,
        waited_ms: 0,
        wait_for: WaitFor::Idle,
        run: None,
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        json!({"task": task_json, "timed_out": true, "waited_ms": 0, "for": "idle", "run": null})
    );
}

#[test]
fn cp_value_shape() {
    let v = CpValue {
        copied: true,
        from: "a".into(),
        to: "b".into(),
        bytes: 12,
        confirmation: "ok".into(),
    };
    assert_eq!(
        serde_json::to_value(&v).unwrap(),
        json!({"copied": true, "from": "a", "to": "b", "bytes": 12, "confirmation": "ok"})
    );
}

#[test]
fn run_value_without_a_sha_serializes_null() {
    let v = RunValue {
        ran: true,
        run: Some(RunId::new(7)),
        exit: Some(0),
        interrupted: false,
        sha: None,
        sync: SyncMode::Full,
        record: None,
        log: "/l".into(),
    };
    let got = serde_json::to_value(&v).unwrap();
    assert_eq!(got.get("sha"), Some(&serde_json::Value::Null));
}
