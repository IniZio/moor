use serde_json::json;

use super::*;

const ASKING: Facts = Facts {
    box_closed: false,
    newest_marker: Some(7),
    run_waits_here: true,
};

#[test]
fn only_the_three_waiting_kinds_add_an_item() {
    for k in ["needs_input", "run_waiting", "run_failed"] {
        assert_eq!(InboxKind::of_event(k).map(InboxKind::as_str), Some(k));
        assert_eq!(InboxKind::parse(k).map(InboxKind::as_str), Some(k));
    }
    for k in ["idled", "turn_done", "closed", "steered"] {
        assert!(InboxKind::of_event(k).is_none(), "{k}");
        assert!(entry_of(k, &json!({}), "t").is_none(), "{k}");
    }
}

#[test]
fn a_question_is_titled_by_its_box() {
    let e = entry_of(
        "needs_input",
        &json!({"question": "Allow npm run e2e?"}),
        "Fix login",
    )
    .unwrap();
    assert_eq!(
        (e.kind, e.title.as_str(), e.text.as_str()),
        (InboxKind::NeedsInput, "Fix login", "Allow npm run e2e?")
    );
    let bare = entry_of("needs_input", &json!({}), "Fix login").unwrap();
    assert_eq!(bare.text, "Needs your input");
}

#[test]
fn a_run_is_titled_by_its_workflow_and_number() {
    let waiting = json!({"workflow": "deps", "run": 14, "question": "Open the PR?", "step": "ok"});
    let e = entry_of("run_waiting", &waiting, "t").unwrap();
    assert_eq!(
        (e.title.as_str(), e.text.as_str()),
        ("deps (run 14)", "Open the PR?")
    );
    let failed = json!({"workflow": "deps", "run": 3, "reason": "bun install exited 1"});
    let e = entry_of("run_failed", &failed, "t").unwrap();
    assert_eq!(
        (e.title.as_str(), e.text.as_str()),
        ("deps (run 3)", "bun install exited 1")
    );
}

#[test]
fn a_question_is_over_once_the_box_moved_on_or_closed() {
    let q = InboxKind::NeedsInput;
    assert!(!is_resolved(q, 7, &ASKING));
    let working = Facts {
        newest_marker: Some(9),
        ..ASKING
    };
    assert!(is_resolved(q, 7, &working));
    let asked_again = Facts {
        newest_marker: Some(9),
        ..ASKING
    };
    assert!(!is_resolved(q, 9, &asked_again));
    let closed = Facts {
        box_closed: true,
        ..ASKING
    };
    assert!(is_resolved(q, 7, &closed));
    let none = Facts {
        newest_marker: None,
        ..ASKING
    };
    assert!(is_resolved(q, 7, &none));
}

#[test]
fn an_approve_wait_is_over_once_its_run_left_it() {
    let w = InboxKind::RunWaiting;
    assert!(!is_resolved(w, 7, &ASKING));
    let left = Facts {
        run_waits_here: false,
        ..ASKING
    };
    assert!(is_resolved(w, 7, &left));
}

#[test]
fn a_failed_run_has_no_wait_to_end() {
    let gone = Facts {
        box_closed: true,
        newest_marker: None,
        run_waits_here: false,
    };
    assert!(!is_resolved(InboxKind::RunFailed, 7, &gone));
}

#[test]
fn resolved_wins_over_read_and_read_over_unread() {
    assert_eq!(state_of(false, false), InboxState::Unread);
    assert_eq!(state_of(true, false), InboxState::Read);
    assert_eq!(state_of(false, true), InboxState::Resolved);
    assert_eq!(state_of(true, true), InboxState::Resolved);
}

#[test]
fn the_count_is_the_unread_items_whose_wait_still_stands() {
    let item = |id, state| InboxItemView {
        id,
        box_name: "acme/web-1".into(),
        repo: None,
        kind: InboxKind::NeedsInput,
        state,
        title: "t".into(),
        text: "q".into(),
        at: "2026-11-18T10:00:00Z".into(),
    };
    let v = InboxValue::of(vec![
        item(4, InboxState::Unread),
        item(3, InboxState::Read),
        item(2, InboxState::Resolved),
        item(1, InboxState::Unread),
    ]);
    assert_eq!(v.unread, 2);
    assert_eq!(v.items.len(), 4);
}
