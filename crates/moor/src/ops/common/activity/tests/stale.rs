//! A settle of a reading taken before another op relaunched the agent writes nothing about the
//! new launch: the store refuses each write made for the launch the reading probed.

use std::sync::Arc;

use moor_core::{BoxId, LaunchId};

use super::{cx, is_working, row, run_once, seen, working_box, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::settle;
use crate::store::LaunchEnd;
use crate::testing::agent::{exited, said};
use crate::testing::engine::FakeEngine;
use crate::testing::inbox::sent;

/// Ends the live launch of `id` as a relaunch does and begins the next one; the new launch.
fn relaunch(cx: &Cx, id: BoxId) -> LaunchId {
    let store = cx.store().unwrap();
    let old = store.live_launch(id).unwrap().unwrap();
    let end = LaunchEnd {
        session: Some("relaunch"),
        ..LaunchEnd::default()
    };
    store.end_launch(id, old.id, &end).unwrap();
    let next = store.open_session(id).unwrap();
    store.start_session(next.id).unwrap();
    store.begin_launch(id, SESSION).unwrap()
}

fn count(cx: &Cx, id: BoxId, kind: &str) -> usize {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

#[test]
fn an_exit_read_of_launch_n_settled_after_a_relaunch_leaves_n_plus_one_working() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, exited(Some(1))));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let reading = run_once(&cx, id);
    let new = relaunch(&cx, id);

    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();

    let store = cx.store().unwrap();
    assert_eq!(store.live_launch(id).unwrap().map(|l| l.id), Some(new));
    assert!(is_working(&row(&cx, id)), "{:?}", row(&cx, id).session);
    assert_eq!(count(&cx, id, "session_ended"), 1, "only the relaunch's");
}

#[test]
fn an_idle_read_of_launch_n_settled_after_a_relaunch_records_no_reply_and_no_idle() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, said("done", seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let reading = run_once(&cx, id);
    relaunch(&cx, id);

    settle(&cx, cx.store().unwrap(), &row(&cx, id), &reading).unwrap();

    assert!(is_working(&row(&cx, id)));
    assert_eq!(count(&cx, id, "agent_said"), 0);
}

#[test]
fn an_exit_read_of_launch_n_does_not_answer_a_steer_sent_to_n_plus_one() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, exited(Some(1))));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let reading = run_once(&cx, id);
    relaunch(&cx, id);
    let store = cx.store().unwrap();
    let row_id = sent(store, id, None, None);

    settle(&cx, store, &row(&cx, id), &reading).unwrap();

    let open: Vec<_> = store.sent(id).unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(open, [row_id]);
    assert_eq!(count(&cx, id, "turn_done"), 0);
}
