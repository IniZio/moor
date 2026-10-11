//! Wire-shape tests for the view structs.

use super::*;
use serde_json::{json, Value};

fn task() -> TaskView {
    TaskView {
        id: 7,
        name: "t".into(),
        repo: "r".into(),
        issue: None,
        goal: Some("g".into()),
        state: TaskState::Working,
        state_reason: None,
        base: None,
        branch: Some("b".into()),
        worktree: None,
        workspace: None,
        r#box: Some(BoxView {
            state: BoxState::Ready,
            provider: "p".into(),
            handle: "h".into(),
            egress: Some("enforced".into()),
            asleep_since: None,
        }),
        agent: None,
        run: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn task_json() -> Value {
    json!({
        "id": 7, "name": "t", "repo": "r", "issue": null, "goal": "g",
        "state": "working", "state_reason": null, "base": null, "branch": "b",
        "worktree": null, "workspace": null,
        "box": {"state": "ready", "provider": "p", "handle": "h",
                "egress": "enforced", "asleep_since": null},
        "agent": null, "run": null, "updated_at": "2026-01-01T00:00:00Z"
    })
}

#[test]
fn task_row_keys_match_design() {
    assert_eq!(serde_json::to_value(task()).ok(), Some(task_json()));
}

#[test]
fn task_without_box_serializes_null() {
    let mut t = task();
    t.r#box = None;
    let v = serde_json::to_value(t).ok();
    assert_eq!(v.and_then(|v| v.get("box").cloned()), Some(Value::Null));
}

#[test]
fn agent_and_run_nested_keys() {
    let mut t = task();
    t.agent = Some(AgentView {
        state: "idle".into(),
        source: "hook".into(),
        waiting_for: None,
    });
    t.run = Some(RunView {
        id: 1,
        argv: vec!["ls".into()],
        started_at: "x".into(),
    });
    let v = serde_json::to_value(t).ok().unwrap_or(Value::Null);
    assert_eq!(
        v.get("agent"),
        Some(&json!({"state": "idle", "source": "hook", "waiting_for": null}))
    );
    assert_eq!(
        v.get("run"),
        Some(&json!({"id": 1, "argv": ["ls"], "started_at": "x"}))
    );
}

#[test]
fn start_view_keys_match_design() {
    let v = StartView {
        task: task(),
        attached: false,
        queued: false,
        promoted: vec!["a".into()],
        setup: SetupView {
            ran: true,
            ok: true,
            line: None,
        },
        dotfiles: DotfilesView::default(),
        agent: AgentLaunchView {
            launched: true,
            session: Some("s".into()),
        },
        warnings_inline: vec![],
    };
    assert_eq!(
        serde_json::to_value(v).ok(),
        Some(json!({
            "task": task_json(), "attached": false, "queued": false, "promoted": ["a"],
            "setup": {"ran": true, "ok": true, "line": null},
            "dotfiles": {"ran": false, "ok": false, "ms": null},
            "agent": {"launched": true, "session": "s"},
            "warnings_inline": []
        }))
    );
}

#[test]
fn show_view_keys_match_design() {
    let v = ShowView {
        tasks: vec![task()],
        task: None,
        events: Some(vec![EventView {
            id: 1,
            kind: "ran".into(),
            payload: json!({"a": 1}),
            ts: "t".into(),
        }]),
        pane: Some(PaneView {
            tail: "x".into(),
            status: "ok".into(),
        }),
        reports: Some(vec![ReportView {
            id: 2,
            kind: "k".into(),
            version: 1,
            title: None,
            path: "p".into(),
        }]),
    };
    assert_eq!(
        serde_json::to_value(v).ok(),
        Some(json!({
            "tasks": [task_json()], "task": null,
            "events": [{"id": 1, "kind": "ran", "payload": {"a": 1}, "ts": "t"}],
            "pane": {"tail": "x", "status": "ok"},
            "reports": [{"id": 2, "kind": "k", "version": 1, "title": null, "path": "p"}]
        }))
    );
}

#[test]
fn show_view_absent_sections_are_null() {
    let v = ShowView {
        tasks: vec![],
        task: None,
        events: None,
        pane: None,
        reports: None,
    };
    assert_eq!(
        serde_json::to_value(v).ok(),
        Some(json!({"tasks": [], "task": null, "events": null, "pane": null, "reports": null}))
    );
}

#[test]
fn close_view_keys_match_design() {
    let v = CloseView {
        task: task(),
        box_deleted: true,
        workspace_closed: false,
        agent_stopped: true,
        transcript: None,
        worktree_kept: true,
        promoted: vec![],
    };
    assert_eq!(
        serde_json::to_value(v).ok(),
        Some(json!({
            "task": task_json(), "box_deleted": true, "workspace_closed": false,
            "agent_stopped": true, "transcript": null, "worktree_kept": true, "promoted": []
        }))
    );
}
