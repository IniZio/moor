use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{BoxHandle, ProviderId, TaskName};

use super::{event_view, rfc3339, task_view};
use crate::adapters::clock::FakeClock;
use crate::store::Store;

#[test]
fn rfc3339_formats_epoch_and_leap_day() {
    assert_eq!(rfc3339(Ts::from_millis(0)), "1970-01-01T00:00:00Z");
    assert_eq!(
        rfc3339(Ts::from_millis(1_709_210_096_999)),
        "2024-02-29T12:34:56Z"
    );
    assert_eq!(rfc3339(Ts::from_millis(-1000)), "1969-12-31T23:59:59Z");
}

#[test]
fn task_view_maps_row_without_box() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(86_400_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewTask {
        name: TaskName::parse("t").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    let fields = Fields {
        goal: Field::Set("g".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(fields)).unwrap();
    let v = task_view(&store.task(id).unwrap().unwrap());
    assert_eq!(
        (v.id, v.name.as_str(), v.repo.as_str()),
        (id.get(), "t", "r")
    );
    assert_eq!(v.goal.as_deref(), Some("g"));
    assert!(v.r#box.is_none() && v.agent.is_none() && v.run.is_none());
    assert_eq!(v.updated_at, "1970-01-02T00:00:00Z");
    let events = store.events(id, 10).unwrap();
    let ev = event_view(events.first().unwrap());
    assert_eq!(ev.id, events.first().unwrap().id);
    assert_eq!(ev.ts, "1970-01-02T00:00:00Z");
}

#[test]
fn task_view_maps_box() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewTask {
        name: TaskName::parse("t").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    store
        .apply(Decision::new(id).set(fields).box_input(BoxInput::Requested))
        .unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        }))
        .unwrap();
    let b = task_view(&store.task(id).unwrap().unwrap()).r#box.unwrap();
    assert_eq!(
        (b.state, b.provider.as_str(), b.handle.as_str()),
        (BoxState::Ready, "p", "h")
    );
    assert!(b.asleep_since.is_none());
}
