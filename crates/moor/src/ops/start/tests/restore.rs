//! A Box that lost its machine resumes its agent's conversation on new hands, from the archive
//! saved after its last finished turn, or starts a fresh agent told the conversation was lost.

use std::sync::Arc;

use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{EngineError, LaunchReq};
use moor_core::BoxId;
use serde_json::Value;

use super::relaunch::{gone, prompt_req, session, set_agent_session};
use super::{req, world, World};
use crate::blobs::BlobKey;
use crate::ops::common::view::rfc3339;
use crate::ops::start::restore::LOST_LINE;
use crate::ops::start::run;
use crate::store::archive::ArchivePointer;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const AGENT: &str = "agent-1";
const TAKEN: Ts = Ts::from_millis(1_700_000_000_000);

/// A Box `demo` whose agent's session `AGENT` lives only on its first hands, `box-1`.
fn started() -> (World, Arc<FakeEngine>, BoxId) {
    started_on(FakeEngine::new())
}

fn started_on(engine: FakeEngine) -> (World, Arc<FakeEngine>, BoxId) {
    let engine = Arc::new(
        engine
            .holding("box-1", AGENT)
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2")))
            .launch_result(Ok(session("p-3"))),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("remember PLUM-42")).unwrap();
    let id = BoxId::new(first.boxed.id);
    set_agent_session(&w, id, AGENT);
    (w, engine, id)
}

/// Saves `bytes` as the Box's archive and points its Session at it.
fn archived(w: &World, id: BoxId, bytes: &[u8]) {
    let store = w.cx.store().unwrap();
    let session = store.live_session(id).unwrap().unwrap().session.unwrap();
    let key = BlobKey::transcript(id, session, "d1");
    w.cx.blobs().unwrap().put(&key, bytes).unwrap();
    pointed(w, id, key.as_str());
}

fn pointed(w: &World, id: BoxId, key: &str) {
    let store = w.cx.store().unwrap();
    let session = store.live_session(id).unwrap().unwrap().session.unwrap();
    let to = ArchivePointer {
        key: key.to_owned(),
        inc: 1,
        at: TAKEN,
    };
    assert!(store.advance_archive(id, session, &to).unwrap());
}

/// The machine of the Box is lost while its agent works.
fn lose_machine(w: &World, engine: &FakeEngine, id: BoxId) {
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    gone(engine);
}

fn last_launch(engine: &FakeEngine) -> LaunchReq {
    engine.launches().pop().unwrap()
}

fn event(w: &World, id: BoxId, kind: &str) -> Option<Value> {
    let store = w.cx.store().unwrap();
    store.newest_event_of(id, kind).unwrap().map(|e| e.payload)
}

fn assert_lost(w: &World, engine: &FakeEngine, id: BoxId, reason: &str) {
    let launch = last_launch(engine);
    assert_eq!(launch.handle.as_str(), "box-2");
    assert_eq!(launch.resume, None);
    assert_eq!(launch.prompt.text(), Some(LOST_LINE));
    let lost = event(w, id, "session_lost").expect("the loss is told");
    let said = lost.get("reason").and_then(Value::as_str).unwrap();
    assert!(said.contains(reason), "{said}");
    assert_eq!(event(w, id, "session_restored"), None);
}

#[test]
fn a_start_after_the_machine_is_lost_restores_the_conversation_before_resuming_it() {
    let (w, engine, id) = started();
    archived(&w, id, b"archive of PLUM-42");
    lose_machine(&w, &engine, id);

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(
        engine.restores(),
        [(
            "box-2".to_owned(),
            AGENT.to_owned(),
            b"archive of PLUM-42".to_vec()
        )]
    );
    assert_eq!(
        engine.resumes(),
        [("box-2".to_owned(), AGENT.to_owned(), true)],
        "the resume finds its conversation"
    );
    assert_eq!(last_launch(&engine).resume.as_deref(), Some(AGENT));
    let told = event(&w, id, "session_restored").expect("the restore is told");
    assert_eq!(told.get("archive_at"), Some(&Value::from(rfc3339(TAKEN))));
}

#[test]
fn a_second_relaunch_on_the_new_hands_restores_nothing() {
    let (w, engine, id) = started();
    archived(&w, id, b"archive");
    lose_machine(&w, &engine, id);
    run(&w.cx, &req("demo")).unwrap();
    set_agent_session(&w, id, AGENT);
    engine.set_probes("p-2", crate::testing::agent::gone());

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(engine.restores().len(), 1);
    let second = [
        ("box-2".to_owned(), AGENT.to_owned(), true),
        ("box-2".to_owned(), AGENT.to_owned(), true),
    ];
    assert_eq!(engine.resumes(), second);
}

#[test]
fn a_relaunch_on_hands_that_hold_the_session_restores_nothing() {
    let (w, engine, id) = started();
    archived(&w, id, b"archive");
    gone(&engine);

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(engine.restores(), []);
    assert_eq!(
        engine.resumes(),
        [("box-1".to_owned(), AGENT.to_owned(), true)]
    );
    assert_eq!(event(&w, id, "session_restored"), None);
    assert_eq!(event(&w, id, "session_lost"), None);
}

#[test]
fn without_an_archive_the_agent_starts_fresh_and_is_told_the_conversation_was_lost() {
    let (w, engine, id) = started();
    lose_machine(&w, &engine, id);

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(engine.restores(), []);
    assert_lost(&w, &engine, id, "no archive");
}

#[test]
fn an_archive_that_cannot_be_fetched_starts_the_agent_fresh_and_is_told() {
    let (w, engine, id) = started();
    pointed(&w, id, "transcripts/1/1/missing.tgz");
    lose_machine(&w, &engine, id);

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(engine.restores(), []);
    assert_lost(&w, &engine, id, "no such blob");
}

#[test]
fn an_archive_that_cannot_be_unpacked_starts_the_agent_fresh_and_is_told() {
    let unpack = EngineError::Transcript("restore unpack: exit 2".to_owned());
    let (w, engine, id) = started_on(FakeEngine::new().fail_restore(unpack));
    archived(&w, id, b"archive");
    lose_machine(&w, &engine, id);

    run(&w.cx, &req("demo")).unwrap();

    assert_eq!(engine.restores().len(), 1);
    assert_lost(&w, &engine, id, "restore unpack: exit 2");
}
