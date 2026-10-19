//! Tests for the event kinds, payloads and step-to-event tables.

use super::*;
use crate::domain::boxes::{BoxInput, BoxState};
use crate::name::BoxHandle;

const PINNED_KINDS: [&str; 20] = [
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

#[allow(clippy::too_many_lines, reason = "one sample per event variant")]
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
        Event::StaysUp { reason: s("r") },
        Event::Ran {
            run: Some(run),
            exit: Some(1),
            interrupted: false,
            error: None,
        },
        Event::Copied {
            from: s("a"),
            to: s("b"),
        },
        Event::SessionStarted { session: s("x") },
        Event::SessionFailed {
            step: s("pane"),
            message: s("m"),
        },
        Event::Relaunched {
            old_pane: Some(s("p-1")),
            new_pane: Some(s("p-2")),
            goal: Some(s("g")),
            cause: None,
        },
        Event::Checked {
            credentials: vec![CheckedCred {
                name: s("k"),
                source: s("user"),
                resolves: true,
            }],
        },
        Event::RunStarted { run },
        Event::Attached,
        Event::SessionsStopped {
            sessions: vec![s("x")],
        },
        Event::AgentPromptBlocked {
            pane: s("p"),
            prompt: s("t"),
            line: s("l"),
        },
        Event::AgentPromptCheckFailed {
            pane: s("p"),
            error: s("e"),
        },
        Event::TranscriptSaved {
            trigger: s("close"),
            path: s("p"),
            digest: s("d"),
        },
        Event::TranscriptFailed {
            trigger: s("close"),
            error: s("e"),
        },
        Event::TranscriptSkipped {
            trigger: s("close"),
            reason: s("r"),
        },
        Event::Steered {
            session: s("x"),
            message: s("m"),
            submit_retried: true,
        },
        Event::ProxyStarted {
            box_port: 1,
            host_port: 2,
            pid: 3,
            handle: s("h"),
        },
        Event::TunnelRestarted {
            box_port: 1,
            reason: s("r"),
        },
        Event::SetupFinished { ms: 5, ok: true },
        Event::EgressRefused {
            host: s("h"),
            rule: Some(s("r")),
            method: None,
            category: None,
        },
        Event::CredentialWithheld {
            host: s("h"),
            rule: s("r"),
            method: s("GET"),
            shape: None,
        },
        Event::ProxyCredentialFailed { rule: s("r") },
        Event::NeedsInput {
            question: Some(s("q")),
            options: vec![s("a")],
        },
        Event::Proof {
            name: s("a.png"),
            size: 3,
            sha256: s("ab"),
            refused: None,
        },
        Event::AgentRateLimited {
            rule: s("r"),
            status: 429,
            resets_at: None,
        },
        Event::ProxyCredentialRejected {
            rule: s("r"),
            status: 401,
        },
        Event::ProxyCredentialRecovered { rule: s("r") },
    ]
}

#[test]
fn asserted_kinds_are_the_frozen_twenty() {
    let mut got = ASSERTED_KINDS.to_vec();
    got.sort_unstable();
    let mut want = PINNED_KINDS.to_vec();
    want.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(ASSERTED_KINDS, PINNED_KINDS);
}

/// Position of an event in `all_events()`; the exhaustive match fails to compile on a new variant.
const fn event_index(e: &Event) -> usize {
    match e {
        Event::Started => 0,
        Event::Rejected { .. } => 1,
        Event::WaitingForBox { .. } => 2,
        Event::SettingUp { .. } => 3,
        Event::Working => 4,
        Event::SetupFailed { .. } => 5,
        Event::CredentialFailed { .. } => 6,
        Event::Idled => 7,
        Event::Busy => 8,
        Event::Asleep { .. } => 9,
        Event::Woke => 10,
        Event::Closed { .. } => 11,
        Event::BoxRequested => 12,
        Event::BoxReady => 13,
        Event::BoxFailed { .. } => 14,
        Event::BoxAsleep => 15,
        Event::BoxGone => 16,
        Event::BoxDeleted => 17,
        Event::BoxReleaseFailed { .. } => 18,
        Event::StaysUp { .. } => 19,
        Event::Ran { .. } => 20,
        Event::Copied { .. } => 21,
        Event::SessionStarted { .. } => 22,
        Event::SessionFailed { .. } => 23,
        Event::Relaunched { .. } => 24,
        Event::Checked { .. } => 25,
        Event::RunStarted { .. } => 26,
        Event::Attached => 27,
        Event::SessionsStopped { .. } => 28,
        Event::AgentPromptBlocked { .. } => 29,
        Event::AgentPromptCheckFailed { .. } => 30,
        Event::TranscriptSaved { .. } => 31,
        Event::TranscriptFailed { .. } => 32,
        Event::TranscriptSkipped { .. } => 33,
        Event::Steered { .. } => 34,
        Event::ProxyStarted { .. } => 35,
        Event::TunnelRestarted { .. } => 36,
        Event::SetupFinished { .. } => 37,
        Event::EgressRefused { .. } => 38,
        Event::CredentialWithheld { .. } => 39,
        Event::ProxyCredentialFailed { .. } => 40,
        Event::NeedsInput { .. } => 41,
        Event::Proof { .. } => 42,
        Event::AgentRateLimited { .. } => 43,
        Event::ProxyCredentialRejected { .. } => 44,
        Event::ProxyCredentialRecovered { .. } => 45,
    }
}

/// The samples cover every event variant once and the kinds are unique.
#[test]
fn event_samples_cover_every_variant_with_unique_kinds() {
    let all = all_events();
    assert_eq!(all.len(), 46);
    for (pos, e) in all.iter().enumerate() {
        assert_eq!(event_index(e), pos, "{e:?}");
    }
    let mut kinds: Vec<_> = all.iter().map(Event::kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), all.len());
}

/// Every asserted kind is a real kind, listed once, and `is_asserted` agrees.
#[test]
fn asserted_kinds_are_real_unique_kinds() {
    let events = all_events();
    let kinds: Vec<_> = events.iter().map(Event::kind).collect();
    for k in ASSERTED_KINDS {
        assert!(kinds.contains(&k), "{k}");
        assert_eq!(ASSERTED_KINDS.iter().filter(|x| **x == k).count(), 1, "{k}");
    }
    for e in &events {
        assert_eq!(e.is_asserted(), PINNED_KINDS.contains(&e.kind()), "{e:?}");
    }
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

mod kinds;
