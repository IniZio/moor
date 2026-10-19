//! Tests for the decision builder and intents.

use super::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange, NewTask, TaskTarget};
use crate::domain::box_state::BoxLifecycleInput;
use crate::domain::event::Event;
use crate::domain::session_state::SessionInput;
use crate::domain::task::{TaskInput, TaskState};
use crate::name::{BoxId, BoxName, RunId};

/// `new` targets an existing task with nothing else set.
#[test]
fn new_is_empty() {
    let d = Decision::new(BoxId::new(3));
    assert_eq!(d.task, TaskTarget::Existing(BoxId::new(3)));
    assert!(d.task_input.is_none() && d.box_input.is_none());
    assert_eq!(d.set, Fields::default());
    assert_eq!(d.intent, IntentChange::Keep);
    assert!(d.events.is_empty());
}

/// `insert` carries the Insert input.
#[test]
fn insert_sets_input() {
    let new = NewTask {
        name: BoxName::parse_allow_bare("a").unwrap(),
        repo: "r".into(),
    };
    let d = Decision::insert(new.clone());
    assert_eq!(d.task, TaskTarget::New(new));
    assert_eq!(d.lifecycle_input, Some(BoxLifecycleInput::Insert));
    assert_eq!(d.task_input, None);
}

/// The Box lifecycle and Session builders set their inputs.
#[test]
fn lifecycle_and_session_builders() {
    let d = Decision::new(BoxId::new(1))
        .lifecycle(BoxLifecycleInput::Wake)
        .session(SessionInput::AgentBusy);
    assert_eq!(d.lifecycle_input, Some(BoxLifecycleInput::Wake));
    assert_eq!(d.session_input, Some(SessionInput::AgentBusy));
    assert_eq!(d.task_input, None);
}

/// Builder methods chain and keep order of events.
#[test]
fn builder_chains() {
    let f = Fields {
        goal: Field::Set("g".into()),
        ..Fields::default()
    };
    let d = Decision::new(BoxId::new(1))
        .task_input(TaskInput::Admitted)
        .set(f.clone())
        .intent(IntentChange::Clear)
        .event(ExtraEvent::RunStarted { run: RunId::new(1) })
        .event(ExtraEvent::SessionStarted {
            session: "s".into(),
        });
    assert_eq!(d.task_input, Some(TaskInput::Admitted));
    assert_eq!(d.set, f);
    assert_eq!(d.intent, IntentChange::Clear);
    assert_eq!(
        d.events,
        vec![
            ExtraEvent::RunStarted { run: RunId::new(1) },
            ExtraEvent::SessionStarted {
                session: "s".into()
            }
        ]
    );
}

/// Intent accessors report kind, pid and run.
#[test]
fn intent_accessors() {
    let p = Intent::Provisioning { pid: 1 };
    let r = Intent::Running {
        run: RunId::new(9),
        pid: 2,
    };
    let s = Intent::Setup { pid: 3 };
    assert_eq!(
        (p.kind(), r.kind(), s.kind()),
        ("provisioning", "running", "setup")
    );
    assert_eq!((p.pid(), r.pid(), s.pid()), (1, 2, 3));
    assert_eq!(
        (p.run(), r.run(), s.run()),
        (None, Some(RunId::new(9)), None)
    );
}

/// Derived kinds are refused and handed back unchanged.
#[test]
fn derived_events_are_refused() {
    for e in [
        Event::Closed {
            from: TaskState::Idle,
        },
        Event::BoxReady,
        Event::BoxFailed {
            error: "c".into(),
            handle: None,
        },
        Event::Idled,
    ] {
        assert_eq!(ExtraEvent::try_from(e.clone()), Err(e));
    }
}

/// Every extra event survives a trip through `Event` with the same kind.
#[test]
fn extra_events_round_trip() {
    let all = [
        ExtraEvent::Ran {
            run: Some(RunId::new(2)),
            exit: Some(1),
            interrupted: false,
            error: None,
        },
        ExtraEvent::Ran {
            run: None,
            exit: None,
            interrupted: false,
            error: None,
        },
        ExtraEvent::Copied {
            from: "a".into(),
            to: "b".into(),
        },
        ExtraEvent::SessionStarted {
            session: "s".into(),
        },
        ExtraEvent::SetupFailed { line: "l".into() },
        ExtraEvent::Checked {
            credentials: vec![crate::domain::event::CheckedCred {
                name: "c".into(),
                source: "user".into(),
                resolves: true,
            }],
        },
        ExtraEvent::RunStarted { run: RunId::new(3) },
        ExtraEvent::StaysUp { reason: "r".into() },
        ExtraEvent::ProxyStarted {
            box_port: 1080,
            host_port: 2080,
            pid: 7,
            handle: "h".into(),
        },
        ExtraEvent::TunnelRestarted {
            box_port: 1080,
            reason: "r".into(),
        },
    ];
    for x in all {
        let e = Event::from(x.clone());
        let kind = e.kind();
        let back = ExtraEvent::try_from(e).unwrap();
        assert_eq!(back, x);
        assert_eq!(Event::from(back).kind(), kind);
    }
}

/// Position of an intent in `intent_samples()`; the exhaustive match fails to compile on a new variant.
const fn intent_index(i: &Intent) -> usize {
    match i {
        Intent::Provisioning { .. } => 0,
        Intent::Running { .. } => 1,
        Intent::Setup { .. } => 2,
    }
}

/// One sample of every intent variant.
fn intent_samples() -> [Intent; 3] {
    [
        Intent::Provisioning { pid: 1 },
        Intent::Running {
            run: RunId::new(2),
            pid: 2,
        },
        Intent::Setup { pid: 3 },
    ]
}

/// The samples cover every intent once, and the kind names are unique.
#[test]
fn intent_samples_cover_every_variant_with_unique_kinds() {
    let all = intent_samples();
    for (pos, i) in all.iter().enumerate() {
        assert_eq!(intent_index(i), pos, "{i:?}");
    }
    let mut kinds: Vec<_> = all.iter().map(Intent::kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), all.len());
}
