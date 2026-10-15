use std::path::Path;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::TaskId;

use super::super::reads::NamedEvent;
use super::super::Store;
use super::{new_task, open};

/// Two tasks with interleaved session events, plus every event the store holds.
fn seeded_events(dir: &Path) -> (Store, TaskId, Vec<NamedEvent>) {
    let (store, _clock) = open(dir);
    let a = new_task(&store, "alpha");
    let b = new_task(&store, "beta");
    for (id, session) in [(a, "s1"), (b, "s2"), (a, "s3")] {
        store
            .apply(Decision::new(id).event(ExtraEvent::SessionStarted {
                session: session.into(),
            }))
            .unwrap();
    }
    let all = store.events_after(0, 1000).unwrap();
    (store, a, all)
}

#[test]
fn events_after_is_ordered_and_named_across_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, a, all) = seeded_events(dir.path());
    assert!(all.len() >= 5);
    assert!(all
        .iter()
        .zip(all.iter().skip(1))
        .all(|(x, y)| x.row.id < y.row.id));
    let names: Vec<_> = all
        .iter()
        .map(|e| (e.row.task == a, e.task_name.as_str()))
        .collect();
    assert!(names.contains(&(true, "alpha")));
    assert!(names.contains(&(false, "beta")));
    assert!(names
        .iter()
        .all(|&(is_a, n)| n == if is_a { "alpha" } else { "beta" }));
}

#[test]
fn events_after_returns_exactly_the_later_events() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _a, all) = seeded_events(dir.path());
    let second = all.get(1).expect("at least two events").row.id;
    let later = store.events_after(second, 1000).unwrap();
    assert_eq!(later, all.iter().skip(2).cloned().collect::<Vec<_>>());
}

#[test]
fn events_after_limit_caps_and_resuming_leaves_no_gap() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _a, all) = seeded_events(dir.path());
    let mut got = Vec::new();
    let mut cursor = 0;
    loop {
        let batch = store.events_after(cursor, 2).unwrap();
        assert!(batch.len() <= 2);
        let Some(last) = batch.last() else { break };
        cursor = last.row.id;
        got.extend(batch);
    }
    assert_eq!(got, all);
    assert!(store.events_after(cursor, 10).unwrap().is_empty());
}
