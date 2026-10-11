use super::*;
use serde_json::json;

#[test]
fn task_state_parse() {
    let cases = [
        ("checking", TaskState::Checking),
        ("rejected", TaskState::Rejected),
        ("waiting_for_box", TaskState::WaitingForBox),
        ("setting_up", TaskState::SettingUp),
        ("working", TaskState::Working),
        ("setup_failed", TaskState::SetupFailed),
        ("idle", TaskState::Idle),
        ("asleep", TaskState::Asleep),
        ("closed", TaskState::Closed),
        ("weird", TaskState::Other("weird".into())),
    ];
    for (s, want) in cases {
        assert_eq!(TaskState::parse(s), want);
    }
}

#[test]
fn box_state_parse() {
    let cases = [
        ("starting", BoxState::Starting),
        ("ready", BoxState::Ready),
        ("failed", BoxState::Failed),
        ("gone", BoxState::Gone),
        ("stopped", BoxState::Stopped),
        ("deleted", BoxState::Deleted),
        ("release_failed", BoxState::ReleaseFailed),
        ("asleep", BoxState::Asleep),
        ("weird", BoxState::Other("weird".into())),
    ];
    for (s, want) in cases {
        assert_eq!(BoxState::parse(s), want);
    }
}

#[test]
fn event_kind_from_old() {
    let cases = [
        ("started", EventKind::Started),
        ("rejected", EventKind::Rejected),
        ("waiting_for_box", EventKind::WaitingForBox),
        ("setting_up", EventKind::SettingUp),
        ("working", EventKind::Working),
        ("setup_failed", EventKind::SetupFailed),
        ("idled", EventKind::Idled),
        ("busy", EventKind::Busy),
        ("asleep", EventKind::Asleep),
        ("woke", EventKind::Woke),
        ("closed", EventKind::Closed),
        ("box_ready", EventKind::BoxReady),
        ("box_failed", EventKind::BoxFailed),
        ("box_gone", EventKind::BoxGone),
        ("box_deleted", EventKind::BoxDeleted),
        ("box_asleep", EventKind::BoxAsleep),
        ("session_started", EventKind::SessionStarted),
        ("ran", EventKind::Ran),
        ("copied", EventKind::Copied),
        ("credential_failed", EventKind::CredentialFailed),
        ("weird", EventKind::Other("weird".into())),
    ];
    for (s, want) in cases {
        assert_eq!(EventKind::from_old(s), want);
    }
}

#[test]
fn exit_class_is_err() {
    assert!(!ExitClass::Ok.is_err());
    assert!(ExitClass::Usage.is_err());
    assert!(ExitClass::Refused.is_err());
    assert!(ExitClass::Failed.is_err());
}

#[test]
fn exit_classes() {
    assert_eq!(exit_class(0), ExitClass::Ok);
    assert_eq!(exit_class(2), ExitClass::Usage);
    assert_eq!(exit_class(1), ExitClass::Failed);
}

#[test]
fn task_view_from_json() {
    let v = json!({
        "id": 1, "name": "t1", "state": "working", "worktree": "/w/t1",
        "workspace": "moor:t1", "workspace_id": "ws-1",
        "box": {"driver": "crabbox", "handle": "local/moor-t1-abc", "state": "ready"}
    });
    let t = TaskView::from_json(&v);
    assert_eq!(t.name, "t1");
    assert_eq!(t.state, TaskState::Working);
    assert_eq!(t.worktree.as_deref(), Some("/w/t1"));
    assert_eq!(t.workspace_id.as_deref(), Some("ws-1"));
    let b = t.boxed.unwrap();
    assert_eq!(b.state, BoxState::Ready);
    assert_eq!(b.handle.as_deref(), Some("local/moor-t1-abc"));

    let t = TaskView::from_json(&json!({"name": "n", "state": "closed", "box": null}));
    assert!(t.boxed.is_none());
}
