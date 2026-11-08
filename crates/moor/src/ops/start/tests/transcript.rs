use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::engine::EngineError;
use moor_core::{BoxId, StartReq};

use super::relaunch::{gone, session, set_agent_session, started};
use super::{req, World};
use crate::ops::start::run;
use crate::store::db::Conn;
use crate::store::db::DbExt as _;
use crate::testing::engine::FakeEngine;
use moor_schema::tables::Launches;
use sea_query::{Asterisk, Expr, ExprTrait as _, Order, Query};

const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let ev = w.cx.store().unwrap().events(id, 100).unwrap();
    ev.into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("transcript_"))
        .collect()
}

fn first_digest(w: &World, id: BoxId) -> Option<String> {
    w.cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .row(
            Query::select()
                .column(Launches::TranscriptDigest)
                .from(Launches::Table)
                .and_where(Expr::col(Launches::Box).eq(id.get()))
                .order_by(Launches::Id, Order::Asc)
                .limit(1),
            |r| r.get(0),
        )
        .unwrap()
}

/// A start of `demo` that resumes the stored agent session, or else gives the prompt again.
fn asked(resume: bool) -> StartReq {
    StartReq {
        prompt: (!resume).then(|| "ship it".to_owned()),
        resume,
        ..req("demo")
    }
}

/// A started world whose engine serves `transcript`; the old session is still live in the hook.
fn with_transcript(engine: FakeEngine) -> (World, Arc<FakeEngine>, BoxId) {
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
        let c = Conn::open_file(&db).unwrap();
        let open: i64 = c
            .row(
                Query::select()
                    .expr(Expr::col(Asterisk).count())
                    .from(Launches::Table)
                    .and_where(Expr::col(Launches::EndedAt).is_null()),
                |r| r.get(0),
            )
            .unwrap();
        seen.store(open == 1, Ordering::SeqCst);
    });
    gone(&engine);
    let v = run(&w.cx, &asked(false)).unwrap();
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
    gone(&engine);
    let v = run(&w.cx, &asked(false)).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(kinds(&w, id), ["transcript_failed"]);
    assert_eq!(first_digest(&w, id), None);
}

#[test]
fn a_resuming_relaunch_skips_the_save_and_makes_no_box_call() {
    let (w, engine, id) = with_transcript(FakeEngine::new().transcript_text("abc"));
    set_agent_session(&w, id, "agent-1");
    gone(&engine);
    crate::ops::start::revive_after_wake(&w.cx, w.cx.store().unwrap(), id).unwrap();
    assert_eq!(engine.launches().len(), 1);
    assert!(engine.transcripts().is_empty());
    assert_eq!(kinds(&w, id), ["transcript_skipped"]);
    let ev = w.cx.store().unwrap().events(id, 100).unwrap();
    let last = ev
        .iter()
        .rev()
        .find(|e| e.kind == "transcript_skipped")
        .unwrap();
    assert_eq!(
        last.payload.get("reason"),
        Some(&serde_json::json!("resumed on the same Box"))
    );
    assert_eq!(first_digest(&w, id), None);
}

#[test]
fn a_relaunch_with_a_new_prompt_still_saves_though_a_session_is_stored() {
    let (w, engine, id) = with_transcript(FakeEngine::new().transcript_text("abc"));
    set_agent_session(&w, id, "agent-1");
    gone(&engine);
    let mut r = req("demo");
    r.prompt = Some("new prompt".into());
    run(&w.cx, &r).unwrap();
    assert_eq!(engine.transcripts().len(), 1);
    assert_eq!(kinds(&w, id), ["transcript_saved"]);
}

#[test]
fn a_resuming_relaunch_on_a_recreated_box_still_saves() {
    let (w, engine, id) = with_transcript(FakeEngine::new().transcript_text("abc"));
    set_agent_session(&w, id, "agent-1");
    let lost = Decision::new(id).box_input(BoxInput::DriverMissing);
    w.cx.store().unwrap().apply(lost).unwrap();
    gone(&engine);
    let v = run(&w.cx, &asked(true)).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.transcripts().len(), 1);
    assert_eq!(kinds(&w, id), ["transcript_saved"]);
}
