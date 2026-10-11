//! Tests for the event kinds, payloads and step-to-event tables.

use super::*;
use crate::name::BoxHandle;

const FROM_OLD: [&str; 20] = [
    "started",
    "rejected",
    "waiting_for_box",
    "setting_up",
    "working",
    "setup_failed",
    "idled",
    "busy",
    "asleep",
    "woke",
    "closed",
    "box_ready",
    "box_failed",
    "box_gone",
    "box_deleted",
    "box_asleep",
    "session_started",
    "ran",
    "copied",
    "credential_failed",
];

fn s(v: &str) -> String {
    v.to_string()
}

fn all_events() -> Vec<Event> {
    let run = RunId::new(7);
    vec![
        Event::Started,
        Event::Rejected { reason: s("r") },
        Event::WaitingForBox { max_boxes: 2 },
        Event::SettingUp {
            from: TaskState::Checking,
        },
        Event::Working,
        Event::SetupFailed { line: s("l") },
        Event::CredentialFailed {
            which: s("w"),
            error: s("e"),
        },
        Event::Idled,
        Event::Busy,
        Event::Asleep { auto: true },
        Event::Woke,
        Event::Closed {
            from: TaskState::Idle,
        },
        Event::BoxRequested,
        Event::BoxReady,
        Event::BoxFailed {
            error: s("c"),
            handle: None,
        },
        Event::BoxAsleep,
        Event::BoxGone,
        Event::BoxDeleted,
        Event::BoxReleaseFailed {
            error: s("c"),
            handle: None,
        },
        Event::Ran {
            run,
            exit: Some(1),
            interrupted: false,
        },
        Event::Copied {
            from: s("a"),
            to: s("b"),
        },
        Event::SessionStarted { session: s("x") },
        Event::Checked {
            credentials: vec![s("k")],
        },
        Event::RunStarted { run },
    ]
}

#[test]
fn asserted_kinds_are_the_frozen_twenty() {
    let mut got = ASSERTED_KINDS.to_vec();
    got.sort_unstable();
    let mut want = FROM_OLD.to_vec();
    want.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(ASSERTED_KINDS, FROM_OLD);
}

#[test]
fn kind_equals_serde_tag_and_payload_has_no_kind() {
    for e in all_events() {
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v.get("kind").and_then(|k| k.as_str()),
            Some(e.kind()),
            "{e:?}"
        );
        let p = e.payload();
        assert!(p.is_object());
        assert!(p.get("kind").is_none(), "{e:?}");
    }
}

#[test]
fn unit_variants_have_empty_payload() {
    for e in [
        Event::Started,
        Event::Working,
        Event::Idled,
        Event::Busy,
        Event::Woke,
        Event::BoxRequested,
        Event::BoxReady,
        Event::BoxAsleep,
        Event::BoxGone,
        Event::BoxDeleted,
    ] {
        assert_eq!(e.payload(), serde_json::json!({}), "{e:?}");
    }
}

#[test]
fn payload_carries_fields() {
    let e = Event::Ran {
        run: RunId::new(7),
        exit: None,
        interrupted: true,
    };
    assert_eq!(
        e.payload(),
        serde_json::json!({"run": 7, "exit": null, "interrupted": true})
    );
    let e = Event::Closed {
        from: TaskState::WaitingForBox,
    };
    assert_eq!(e.payload(), serde_json::json!({"from": "waiting_for_box"}));
}

#[test]
fn is_asserted_matches_list() {
    assert!(Event::Started.is_asserted());
    assert!(!Event::BoxRequested.is_asserted());
    assert!(!Event::Checked {
        credentials: vec![]
    }
    .is_asserted());
    assert!(!Event::RunStarted { run: RunId::new(1) }.is_asserted());
    assert!(!Event::BoxReleaseFailed {
        error: s("c"),
        handle: None
    }
    .is_asserted());
    let n = all_events().iter().filter(|e| e.is_asserted()).count();
    assert_eq!(n, 20);
}

#[test]
fn task_events_table() {
    use TaskState as T;
    let ev = |from, to, input: TaskInput| task_events(from, to, &input);
    assert_eq!(
        ev(None, T::Checking, TaskInput::Insert),
        vec![Event::Started]
    );
    assert_eq!(
        ev(
            Some(T::Checking),
            T::Rejected,
            TaskInput::Rejected { reason: s("no") }
        ),
        vec![Event::Rejected { reason: s("no") }]
    );
    assert_eq!(
        ev(
            Some(T::Checking),
            T::WaitingForBox,
            TaskInput::AtCapacity { max_boxes: 3 }
        ),
        vec![Event::WaitingForBox { max_boxes: 3 }]
    );
    for (from, input) in [
        (T::Checking, TaskInput::Admitted),
        (T::WaitingForBox, TaskInput::Promoted),
        (T::SetupFailed, TaskInput::Retry),
    ] {
        assert_eq!(
            ev(Some(from), T::SettingUp, input),
            vec![Event::SettingUp { from }]
        );
    }
    let ready = |setup| TaskInput::BoxReady { setup };
    assert_eq!(
        ev(Some(T::SettingUp), T::Working, ready(SetupOutcome::Ok)),
        vec![Event::Working]
    );
    assert_eq!(
        ev(
            Some(T::SettingUp),
            T::Working,
            ready(SetupOutcome::Failed { line: s("boom") })
        ),
        vec![Event::Working, Event::SetupFailed { line: s("boom") }]
    );
    task_events_table_failures();
}

#[test]
fn task_events_table_failures() {
    use TaskState as T;
    let ev = |from, to, input: TaskInput| task_events(from, to, &input);
    assert_eq!(
        ev(
            Some(T::SettingUp),
            T::SetupFailed,
            TaskInput::AcquireFailed { cause: s("c") }
        ),
        vec![Event::SetupFailed { line: s("c") }]
    );
    assert_eq!(
        ev(
            Some(T::SettingUp),
            T::SetupFailed,
            TaskInput::DotfilesFailed { error: s("d") }
        ),
        vec![Event::SetupFailed { line: s("d") }]
    );
    assert_eq!(
        ev(
            Some(T::SettingUp),
            T::SetupFailed,
            TaskInput::CredFailed {
                which: s("w"),
                error: s("e")
            }
        ),
        vec![
            Event::CredentialFailed {
                which: s("w"),
                error: s("e")
            },
            Event::SetupFailed { line: s("e") }
        ]
    );
    task_events_table_activity();
}

#[test]
fn task_events_table_activity() {
    use TaskState as T;
    let ev = |from, to, input: TaskInput| task_events(from, to, &input);
    assert_eq!(
        ev(Some(T::Working), T::Idle, TaskInput::AgentQuiet),
        vec![Event::Idled]
    );
    assert_eq!(
        ev(Some(T::Idle), T::Working, TaskInput::AgentBusy),
        vec![Event::Busy]
    );
    assert_eq!(
        ev(Some(T::Idle), T::Working, TaskInput::RunStarted),
        vec![Event::Busy]
    );
    assert_eq!(
        ev(Some(T::Idle), T::Asleep, TaskInput::SleepDue),
        vec![Event::Asleep { auto: true }]
    );
    assert_eq!(
        ev(Some(T::Idle), T::Asleep, TaskInput::SleepCmd),
        vec![Event::Asleep { auto: false }]
    );
    assert_eq!(
        ev(Some(T::Asleep), T::Working, TaskInput::Wake),
        vec![Event::Woke]
    );
    assert_eq!(
        ev(Some(T::Idle), T::Closed, TaskInput::Close),
        vec![Event::Closed { from: T::Idle }]
    );
    assert_eq!(
        ev(None, T::Closed, TaskInput::Close),
        vec![Event::Closed { from: T::Closed }]
    );
}

#[test]
fn box_events_table() {
    use BoxState as B;
    let ev = |from, to, input: BoxInput| box_events(from, to, &input, None);
    assert_eq!(
        ev(None, B::Starting, BoxInput::Requested),
        vec![Event::BoxRequested]
    );
    assert_eq!(
        ev(
            Some(B::Starting),
            B::Ready,
            BoxInput::Acquired {
                handle: BoxHandle::new("h")
            }
        ),
        vec![Event::BoxReady]
    );
    assert_eq!(
        ev(Some(B::Asleep), B::Ready, BoxInput::Wake),
        vec![Event::BoxReady]
    );
    assert_eq!(
        ev(
            Some(B::Starting),
            B::Failed,
            BoxInput::AcquireFailed {
                cause: s("c"),
                attempted: None
            }
        ),
        vec![Event::BoxFailed {
            error: s("c"),
            handle: None
        }]
    );
    assert_eq!(
        ev(Some(B::Ready), B::Asleep, BoxInput::Sleep),
        vec![Event::BoxAsleep]
    );
    assert_eq!(
        ev(Some(B::Ready), B::Gone, BoxInput::DriverMissing),
        vec![Event::BoxGone]
    );
    assert_eq!(
        ev(Some(B::Gone), B::Deleted, BoxInput::Released),
        vec![Event::BoxDeleted]
    );
    assert_eq!(
        ev(
            Some(B::Gone),
            B::ReleaseFailed,
            BoxInput::ReleaseErr { cause: s("c") }
        ),
        vec![Event::BoxReleaseFailed {
            error: s("c"),
            handle: None
        }]
    );
}

#[test]
fn box_failure_events_carry_handle() {
    let h = BoxHandle::new("p/x");
    assert_eq!(
        box_events(
            Some(BoxState::Ready),
            BoxState::ReleaseFailed,
            &BoxInput::ReleaseErr { cause: s("c") },
            Some(&h)
        ),
        vec![Event::BoxReleaseFailed {
            error: s("c"),
            handle: Some(s("p/x"))
        }]
    );
    let failed = BoxInput::AcquireFailed {
        cause: s("c"),
        attempted: Some(h),
    };
    assert_eq!(
        box_events(Some(BoxState::Starting), BoxState::Failed, &failed, None),
        vec![Event::BoxFailed {
            error: s("c"),
            handle: Some(s("p/x"))
        }]
    );
}
