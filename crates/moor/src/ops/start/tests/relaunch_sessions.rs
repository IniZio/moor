use moor_core::{BoxId, StartReq};

use super::relaunch::{gone, obs, started};
use super::req;
use crate::ops::start::run;

/// Each Session's ordinal, state and the launch rows linked to it, by ordinal.
fn sessions(w: &super::World, id: BoxId) -> Vec<(i64, String, i64)> {
    let store = w.cx.store().unwrap();
    let conn = store.reader().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT s.ordinal, s.state, (SELECT COUNT(*) FROM launches l WHERE l.session = s.id) \
             FROM sessions s WHERE s.box = ? ORDER BY s.ordinal",
        )
        .unwrap();
    let rows = stmt
        .query_map([id.get()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

#[test]
fn a_first_launch_uses_the_only_session() {
    let (w, _engine, id) = started();
    assert_eq!(sessions(&w, id), [(1, "working".to_owned(), 1)]);
}

#[test]
fn a_relaunch_ends_the_session_and_opens_the_next() {
    let (w, _engine, id) = started();
    w.surface.push_pane_obs("p-1", Ok(obs(Some(3))));
    let again = moor_core::StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &again).unwrap();
    assert_eq!(
        sessions(&w, id),
        [(1, "ended".to_owned(), 1), (2, "working".to_owned(), 1)]
    );
    let store = w.cx.store().unwrap();
    let causes: Vec<String> = store
        .reader()
        .unwrap()
        .prepare("SELECT payload FROM events WHERE box = ? AND kind = 'session_ended'")
        .unwrap()
        .query_map([id.get()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(causes.len(), 1);
    assert!(causes.iter().all(|c| c.contains("relaunch")), "{causes:?}");
}

#[test]
fn a_relaunch_carries_a_new_nonce() {
    let (w, engine, _) = started();
    gone(&w);
    let again = StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    };
    run(&w.cx, &again).unwrap();
    let [first, second] = engine.launches().try_into().unwrap();
    assert_ne!(first.nonce, second.nonce);
}
