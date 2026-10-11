//! Tests for the decision builder and intents.

use super::{Decision, Field, Fields, Intent, IntentChange, NewTask, TaskTarget};
use crate::domain::event::Event;
use crate::domain::task::TaskInput;
use crate::name::{RunId, TaskId, TaskName};

/// `new` targets an existing task with nothing else set.
#[test]
fn new_is_empty() {
    let d = Decision::new(TaskId::new(3));
    assert_eq!(d.task, TaskTarget::Existing(TaskId::new(3)));
    assert!(d.task_input.is_none() && d.box_input.is_none());
    assert_eq!(d.set, Fields::default());
    assert_eq!(d.intent, IntentChange::Keep);
    assert!(d.events.is_empty());
}

/// `insert` carries the Insert input.
#[test]
fn insert_sets_input() {
    let new = NewTask {
        name: TaskName::parse("a").unwrap(),
        repo: "r".into(),
    };
    let d = Decision::insert(new.clone());
    assert_eq!(d.task, TaskTarget::New(new));
    assert_eq!(d.task_input, Some(TaskInput::Insert));
}

/// Builder methods chain and keep order of events.
#[test]
fn builder_chains() {
    let f = Fields {
        goal: Field::Set("g".into()),
        ..Fields::default()
    };
    let d = Decision::new(TaskId::new(1))
        .task_input(TaskInput::Admitted)
        .set(f.clone())
        .intent(IntentChange::Clear)
        .event(Event::Working)
        .event(Event::Idled);
    assert_eq!(d.task_input, Some(TaskInput::Admitted));
    assert_eq!(d.set, f);
    assert_eq!(d.intent, IntentChange::Clear);
    assert_eq!(d.events, vec![Event::Working, Event::Idled]);
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
