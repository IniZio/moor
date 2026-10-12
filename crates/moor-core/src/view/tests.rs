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
            handle: Some("h".into()),
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

fn env_github() -> super::env::EnvGithub {
    use super::env::*;
    EnvGithub {
        identity: "auto".into(),
        app_id: None,
        login: "logged_out".into(),
        mint: EnvMint {
            status: "ok".into(),
            error: None,
            reason: None,
        },
        resolved: EnvResolved {
            app_installed: "n/a".into(),
            ..EnvResolved::default()
        },
    }
}

fn env_item(kind: super::env::EnvKind, name: &str, detail: &str) -> super::env::EnvItem {
    super::env::EnvItem {
        kind,
        name: name.into(),
        source: crate::config::Layer::Repo,
        detail: detail.into(),
        resolves: None,
        error: None,
        delivery: None,
    }
}

fn env_items() -> Vec<super::env::EnvItem> {
    use super::env::EnvKind as K;
    let mut cred = env_item(K::Credential, "API_KEY", "env:API_KEY");
    cred.resolves = Some(false);
    cred.error = Some("unset".into());
    cred.delivery = Some("env".into());
    let mut mcp = env_item(K::Mcp, "srv", "npx srv");
    mcp.resolves = Some(true);
    vec![
        env_item(K::Egress, "allowlist", "only listed"),
        env_item(K::Box, "provider", "docker"),
        env_item(K::Box, "size", "large"),
        cred,
        mcp,
        env_item(K::Setup, "make", "make"),
    ]
}

fn egress() -> super::env::EnvEgress {
    super::env::EnvEgress {
        mode: "allowlist".into(),
        hosts: vec!["a.com".into()],
        note: "only listed".into(),
        enforce: Some("strict".into()),
    }
}

fn settings(user: &str, repo: Option<&str>) -> crate::config::TaskSettings {
    let user = crate::config::parse_user("user.yaml", user).unwrap();
    let repo = repo.map(|r| crate::config::parse_repo(".moor.yaml", r).unwrap());
    crate::config::TaskSettings::resolve(&user, repo.as_ref(), &crate::config::Flags::default())
}

fn env_view() -> super::env::EnvView {
    super::env::EnvView::new(
        "/r".into(),
        &settings(
            "box: {provider: docker, size: medium}",
            Some("box: {size: large}"),
        ),
        env_github(),
        env_items(),
        egress(),
    )
}

#[test]
fn env_view_round_trips_json() {
    let back: super::env::EnvView =
        serde_json::from_value(serde_json::to_value(env_view()).unwrap()).unwrap();
    assert_eq!(back, env_view());
}

#[test]
fn env_view_check_of_matches_kind_and_name() {
    let v = env_view();
    let row = |i: usize| v.rows.get(i).unwrap();
    let c = v.check_of(row(3)).unwrap();
    assert_eq!((c.ok, c.reason.as_deref()), (false, Some("unset")));
    assert!(v.check_of(row(4)).unwrap().ok);
    assert!(v.check_of(row(5)).is_none());
}

#[test]
fn env_view_json_shape() {
    let j = serde_json::to_value(env_view()).unwrap();
    let mut keys: Vec<_> = j.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent",
            "credentials",
            "egress",
            "github",
            "provider",
            "repo",
            "rows"
        ]
    );
    assert_eq!(j.pointer("/agent/model"), Some(&Value::Null));
    assert_eq!(j.pointer("/github/app_id"), Some(&Value::Null));
    assert_eq!(j.pointer("/github/resolved/mode"), Some(&Value::Null));
    assert_eq!(j.pointer("/github/mint"), Some(&json!({"status": "ok"})));
}

#[test]
fn env_view_rows_credentials_egress_json() {
    let j = serde_json::to_value(env_view()).unwrap();
    assert_eq!(j.pointer("/rows/0/delivery"), Some(&Value::Null));
    assert_eq!(j.pointer("/rows/3/delivery"), Some(&json!("env")));
    assert_eq!(j.pointer("/credentials/1/reason"), Some(&Value::Null));
    assert_eq!(j.pointer("/credentials/0/reason"), Some(&json!("unset")));
    assert_eq!(
        j.pointer("/egress").unwrap(),
        &json!({"mode": "allowlist", "hosts": ["a.com"], "note": "only listed", "enforce": "strict"})
    );
    assert_eq!(
        j.pointer("/provider").unwrap(),
        &json!({"name": "docker", "source": "user", "size": "large", "size_source": "repo"})
    );
}

#[test]
fn env_view_missing_facts_default() {
    let bare = super::env::EnvView::new(
        "/r".into(),
        &settings("", None),
        env_github(),
        vec![],
        egress(),
    );
    let j = serde_json::to_value(bare).unwrap();
    assert_eq!(j.pointer("/agent/model"), Some(&Value::Null));
    assert_eq!(j.pointer("/provider/size_source"), Some(&json!("default")));
}

#[test]
fn env_row_value_kind() {
    let v = env_view();
    let kinds: Vec<_> = v.rows.iter().map(|r| r.value_kind).collect();
    assert_eq!(
        kinds
            .get(3)
            .copied()
            .unwrap_or(super::env::ValueKind::Plain),
        super::env::ValueKind::Ref
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == super::env::ValueKind::Ref)
            .count(),
        1
    );
    let j = serde_json::to_value(&v).unwrap();
    assert_eq!(j.pointer("/rows/3/value_kind"), Some(&json!("ref")));
    assert_eq!(j.pointer("/rows/5/value_kind"), Some(&json!("plain")));
}
