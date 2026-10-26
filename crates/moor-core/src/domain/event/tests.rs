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
            from: BoxLifecycle::Checking,
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
            from: BoxLifecycle::Ready,
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
        Event::SessionEnded {
            session: 3,
            cause: s("exited"),
            exit: Some(0),
        },
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
            reason: None,
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
        Event::TurnDone {
            steer: 1,
            outcome: TurnOutcome::Idle,
        },
        Event::ArtifactPublished {
            name: s("shot.png"),
            version: 1,
            artifact_kind: s("screenshot"),
            content_type: s("image/png"),
            size: 12,
            sha256: s("ab"),
        },
        Event::EgressAborted {
            host: s("h"),
            rule: None,
        },
        Event::Cloned { ms: 7 },
        Event::AgentSaid { text: s("done") },
        Event::LaunchTimedOut { after_secs: 180 },
        Event::RelaunchCapped {
            count: 3,
            window_secs: 600,
        },
        Event::AgentDoing {
            text: s("Bash cargo test"),
        },
        Event::AgentLinkLost {
            pane: Some("%1".to_owned()),
            exit: Some(137),
        },
        Event::AgentLinkRestored {
            pane: "%2".to_owned(),
        },
        Event::AgentUnreachable {
            attempts: 4,
            error: "timed out".to_owned(),
        },
        Event::Machine(MachineEvent::MachineAttached {
            name: "mini".to_owned(),
            ports: vec![8081],
        }),
        Event::Machine(MachineEvent::MachineLeftOut {
            labels: vec!["ios-sim".to_owned()],
            reason: "you have no machine with labels ios-sim".to_owned(),
        }),
        Event::Machine(MachineEvent::MachineForwardFailed {
            port: Some(8081),
            error: "exit 255".to_owned(),
        }),
        Event::Machine(MachineEvent::MachineDetached {
            name: "mini".to_owned(),
            asleep: true,
        }),
        Event::Machine(MachineEvent::MachineWaiting {
            name: "mini".to_owned(),
            reason: "machine mini is attached to Box b".to_owned(),
        }),
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
#[allow(clippy::too_many_lines, reason = "one arm per event variant")]
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
        Event::SessionEnded { .. } => 23,
        Event::SessionFailed { .. } => 24,
        Event::Relaunched { .. } => 25,
        Event::Checked { .. } => 26,
        Event::RunStarted { .. } => 27,
        Event::Attached => 28,
        Event::SessionsStopped { .. } => 29,
        Event::AgentPromptBlocked { .. } => 30,
        Event::AgentPromptCheckFailed { .. } => 31,
        Event::TranscriptSaved { .. } => 32,
        Event::TranscriptFailed { .. } => 33,
        Event::TranscriptSkipped { .. } => 34,
        Event::Steered { .. } => 35,
        Event::ProxyStarted { .. } => 36,
        Event::TunnelRestarted { .. } => 37,
        Event::SetupFinished { .. } => 38,
        Event::EgressRefused { .. } => 39,
        Event::CredentialWithheld { .. } => 40,
        Event::ProxyCredentialFailed { .. } => 41,
        Event::NeedsInput { .. } => 42,
        Event::AgentRateLimited { .. } => 43,
        Event::ProxyCredentialRejected { .. } => 44,
        Event::ProxyCredentialRecovered { .. } => 45,
        Event::TurnDone { .. } => 46,
        Event::ArtifactPublished { .. } => 47,
        Event::EgressAborted { .. } => 48,
        Event::Cloned { .. } => 49,
        Event::AgentSaid { .. } => 50,
        Event::LaunchTimedOut { .. } => 51,
        Event::RelaunchCapped { .. } => 52,
        Event::AgentDoing { .. } => 53,
        Event::AgentLinkLost { .. } => 54,
        Event::AgentLinkRestored { .. } => 55,
        Event::AgentUnreachable { .. } => 56,
        Event::Machine(m) => match m {
            MachineEvent::MachineAttached { .. } => 57,
            MachineEvent::MachineLeftOut { .. } => 58,
            MachineEvent::MachineForwardFailed { .. } => 59,
            MachineEvent::MachineDetached { .. } => 60,
            MachineEvent::MachineWaiting { .. } => 61,
        },
    }
}

/// The samples cover every event variant once and the kinds are unique.
#[test]
fn event_samples_cover_every_variant_with_unique_kinds() {
    let all = all_events();
    assert_eq!(all.len(), 62);
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

/// `SessionEnded` names itself and carries the documented payload.
#[test]
fn session_ended_kind_and_payload() {
    let e = Event::SessionEnded {
        session: 3,
        cause: s("exited"),
        exit: None,
    };
    assert_eq!(e.kind(), "session_ended");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind":"session_ended","session":3,"cause":"exited","exit":null})
    );
    assert_eq!(
        e.payload(),
        serde_json::json!({"session":3,"cause":"exited","exit":null})
    );
}

/// `TurnDone` names itself and carries the steer id and each outcome.
#[test]
fn turn_done_kind_and_payload() {
    for (outcome, name) in [
        (TurnOutcome::Idle, "idle"),
        (TurnOutcome::NeedsInput, "needs_input"),
        (TurnOutcome::Exited, "exited"),
    ] {
        let e = Event::TurnDone { steer: 7, outcome };
        assert_eq!(e.kind(), "turn_done");
        assert_eq!(e.payload(), serde_json::json!({"steer":7,"outcome":name}));
    }
}

/// `ArtifactPublished` carries its artifact kind as `artifact_kind`, since `kind` is the tag.
#[test]
fn artifact_published_kind_and_payload() {
    let e = Event::ArtifactPublished {
        name: s("shot.png"),
        version: 2,
        artifact_kind: s("screenshot"),
        content_type: s("image/png"),
        size: 12,
        sha256: s("ab"),
    };
    assert_eq!(e.kind(), "artifact_published");
    assert_eq!(
        e.payload(),
        serde_json::json!({"name":"shot.png","version":2,"artifact_kind":"screenshot","content_type":"image/png","size":12,"sha256":"ab"})
    );
}

mod kinds;

#[test]
fn machine_events_carry_their_own_kind() {
    let e = Event::Machine(MachineEvent::MachineForwardFailed {
        port: Some(8081),
        error: "refused".to_owned(),
    });
    assert_eq!(e.kind(), "machine_forward_failed");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind": "machine_forward_failed", "port": 8081, "error": "refused"})
    );
    assert_eq!(
        e.payload(),
        serde_json::json!({"port": 8081, "error": "refused"})
    );
}
