use std::path::Path;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::{BoxId, SessionId};

use super::super::reads::NamedEvent;
use super::super::Store;
use super::{new_box, open};

/// Two boxes with interleaved session events, plus every event the store holds.
fn seeded_events(dir: &Path) -> (Store, BoxId, Vec<NamedEvent>) {
    let (store, _clock) = open(dir);
    let a = new_box(&store, "alpha");
    let b = new_box(&store, "beta");
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
fn events_after_is_ordered_and_named_across_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, a, all) = seeded_events(dir.path());
    assert!(all.len() >= 5);
    assert!(all
        .iter()
        .zip(all.iter().skip(1))
        .all(|(x, y)| x.row.id < y.row.id));
    let names: Vec<_> = all
        .iter()
        .map(|e| (e.row.box_id == a, e.box_name.as_str()))
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

#[test]
fn events_after_names_the_session_started_at_or_before_each_event() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "alpha");
    let raw = rusqlite::Connection::open(dir.path().join("state").join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO events (box, kind, payload, ts) VALUES (?1, 'early', '{}', 0), (?1, 'late', '{}', 2000000)",
        [id.get()],
    )
    .unwrap();
    let session: i64 = raw
        .query_row("SELECT id FROM sessions WHERE box = ?1", [id.get()], |r| {
            r.get(0)
        })
        .unwrap();
    let all = store.events_after(0, 1000).unwrap();
    let find = |kind: &str| all.iter().find(|e| e.row.kind == kind).unwrap();
    assert_eq!(find("early").session, None);
    assert_eq!(find("late").session, Some(SessionId::new(session)));
}

#[test]
fn events_after_names_the_old_session_for_its_end_in_a_same_millisecond_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "alpha");
    let old = store.box_sessions(id).unwrap().first().unwrap().id;
    assert!(store.end_current_session(id, "exit", Some(0)).unwrap());
    let new = store.open_session(id).unwrap().id;
    store
        .apply(Decision::new(id).event(ExtraEvent::SessionStarted {
            session: "again".into(),
        }))
        .unwrap();
    let all = store.events_after(0, 1000).unwrap();
    let find = |kind: &str| all.iter().find(|e| e.row.kind == kind).unwrap();
    assert_eq!(find("session_ended").session, Some(SessionId::new(old)));
    assert_eq!(find("session_started").session, Some(SessionId::new(new)));
}

#[test]
fn events_after_puts_an_event_at_a_session_start_in_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "alpha");
    let raw = rusqlite::Connection::open(dir.path().join("state").join("moor.db")).unwrap();
    let (session, started): (i64, i64) = raw
        .query_row(
            "SELECT id, started_at FROM sessions WHERE box = ?1",
            [id.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    raw.execute(
        "INSERT INTO events (box, kind, payload, ts) VALUES (?1, 'edge', '{}', ?2)",
        (id.get(), started),
    )
    .unwrap();
    let all = store.events_after(0, 1000).unwrap();
    let edge = all.iter().find(|e| e.row.kind == "edge").unwrap();
    assert_eq!(edge.session, Some(SessionId::new(session)));
}
