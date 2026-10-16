use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::seam::engine::EngineError;
use moor_core::TaskId;

use super::relaunch::{gone, session, started};
use super::{req, World};
use crate::ops::start::run;
use crate::testing::engine::FakeEngine;

const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn kinds(w: &World, id: TaskId) -> Vec<String> {
    let ev = w.cx.store().unwrap().events(id, 100).unwrap();
    ev.into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("transcript_"))
        .collect()
}

fn first_digest(w: &World, id: TaskId) -> Option<String> {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT transcript_digest FROM sessions WHERE task = ? ORDER BY id LIMIT 1",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

/// A started world whose engine serves `transcript`; the old session is still live in the hook.
fn with_transcript(engine: FakeEngine) -> (World, Arc<FakeEngine>, TaskId) {
    let (mut w, _, id) = started();
    let engine = Arc::new(engine.launch_result(Ok(session("p-2"))));
    w.cx = w.cx.with_engine(engine.clone());
    let store = w.cx.store().unwrap();
    let current = store.live_session(id).unwrap().unwrap();
    store.set_registered_session(current.id, "agent-1").unwrap();
    (w, engine, id)
}

#[test]
fn relaunch_saves_the_transcript_before_ending_the_old_session() {
    let (w, engine, id) = with_transcript(FakeEngine::new().transcript_text("abc"));
    let db = w.dir.path().join("moor.db");
    let live_at_save = Arc::new(AtomicBool::new(false));
    let seen = live_at_save.clone();
    engine.on_transcript(move || {
        let c = rusqlite::Connection::open(&db).unwrap();
        let open: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        seen.store(open == 1, Ordering::SeqCst);
    });
    gone(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    assert!(live_at_save.load(Ordering::SeqCst));
    assert_eq!(engine.transcripts().len(), 1);
    assert_eq!(kinds(&w, id), ["transcript_saved"]);
    assert_eq!(first_digest(&w, id).as_deref(), Some(SHA_ABC));
}

#[test]
fn a_failed_save_records_the_failure_and_the_relaunch_goes_on() {
    let (w, engine, id) = with_transcript(
        FakeEngine::new().fail_transcript(EngineError::Transcript("copy: boom".into())),
    );
    gone(&w);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(kinds(&w, id), ["transcript_failed"]);
    assert_eq!(first_digest(&w, id), None);
}
