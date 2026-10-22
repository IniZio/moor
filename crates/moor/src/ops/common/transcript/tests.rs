use std::fs;
use std::os::unix::fs::PermissionsExt;

use moor_core::decide::transcript::Trigger;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::EngineError;
use moor_core::{BoxName, ProviderId};
use serde_json::json;

use super::save;
use crate::cx::Cx;
use crate::store::{BoxRow, SessionRow};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use std::sync::Arc;

fn fx(engine: FakeEngine) -> (tempfile::TempDir, Cx) {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(Arc::new(engine));
    (dir, cx)
}

fn ready(cx: &Cx, dir: &std::path::Path) -> (BoxRow, SessionRow) {
    let (row, session) = unregistered(cx, dir);
    let store = cx.store().unwrap();
    store.set_registered_session(session.id, "agent-1").unwrap();
    let session = store.live_session(row.id).unwrap().unwrap();
    (row, session)
}

fn unregistered(cx: &Cx, dir: &std::path::Path) -> (BoxRow, SessionRow) {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: dir.display().to_string(),
        }))
        .unwrap()
        .box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        box_workdir: Field::Set("/work/box-1".into()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: moor_core::BoxHandle::new("box-1"),
        });
    store.apply(d).unwrap();
    store.begin_session(id, "s-1", Some("p-1")).unwrap();
    (
        store.box_row(id).unwrap().unwrap(),
        store.live_session(id).unwrap().unwrap(),
    )
}

#[test]
fn saves_mode_0600_and_records_the_path_and_digest() {
    let (dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (row, session) = ready(&cx, dir.path());
    let saved = save(&cx, cx.store().unwrap(), &row, &session, Trigger::Close).unwrap();
    let want = dir
        .path()
        .join(format!("transcripts/{}/{}-1.tgz", row.id.get(), session.id));
    assert_eq!(saved.path, want.display().to_string());
    assert_eq!(
        saved.digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        fs::metadata(&want).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(want.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let ev = cx.store().unwrap().events(row.id, 100).unwrap();
    let last = ev.last().unwrap();
    assert_eq!(last.kind, "transcript_saved");
    assert_eq!(last.payload.get("trigger"), Some(&json!("close")));
}

#[test]
fn a_second_save_takes_the_next_number() {
    let (dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (row, session) = ready(&cx, dir.path());
    let store = cx.store().unwrap();
    save(&cx, store, &row, &session, Trigger::Relaunch).unwrap();
    let again = save(&cx, store, &row, &session, Trigger::Close).unwrap();
    assert!(again.path.ends_with(&format!("{}-2.tgz", session.id)));
}

#[test]
fn a_failure_records_the_error_and_saves_nothing() {
    let engine =
        FakeEngine::new().fail_transcript(EngineError::Transcript("archive: exit 3".into()));
    let (dir, cx) = fx(engine);
    let (row, session) = ready(&cx, dir.path());
    let got = save(&cx, cx.store().unwrap(), &row, &session, Trigger::Relaunch);
    assert!(got.is_none());
    let ev = cx.store().unwrap().events(row.id, 100).unwrap();
    let last = ev.last().unwrap();
    assert_eq!(last.kind, "transcript_failed");
    assert_eq!(last.payload.get("trigger"), Some(&json!("relaunch")));
    assert_eq!(
        last.payload.get("error"),
        Some(&json!("transcript: archive: exit 3"))
    );
}

#[test]
fn a_box_that_is_not_ready_is_skipped() {
    let (dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (mut row, session) = ready(&cx, dir.path());
    row.boxed.as_mut().unwrap().state = BoxState::Asleep;
    assert!(save(&cx, cx.store().unwrap(), &row, &session, Trigger::Close).is_none());
    assert!(cx
        .store()
        .unwrap()
        .events(row.id, 100)
        .unwrap()
        .iter()
        .all(|e| !e.kind.starts_with("transcript_")));
}

#[test]
fn nothing_to_archive_records_a_skip_not_a_failure() {
    let engine = FakeEngine::new().fail_transcript(EngineError::NothingToArchive);
    let (dir, cx) = fx(engine);
    let (row, session) = ready(&cx, dir.path());
    assert!(save(&cx, cx.store().unwrap(), &row, &session, Trigger::Relaunch).is_none());
    let ev = cx.store().unwrap().events(row.id, 100).unwrap();
    let last = ev.last().unwrap();
    assert_eq!(last.kind, "transcript_skipped");
    assert_eq!(last.payload.get("trigger"), Some(&json!("relaunch")));
    assert_eq!(
        last.payload.get("reason"),
        Some(&json!("nothing to archive"))
    );
    assert!(ev.iter().all(|e| e.kind != "transcript_failed"));
}

#[test]
fn a_session_without_a_registered_agent_session_is_skipped_without_asking_the_engine() {
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let (dir, cx) = fx(FakeEngine::new());
    let cx = cx.with_engine(engine.clone());
    let (row, session) = unregistered(&cx, dir.path());
    assert!(save(&cx, cx.store().unwrap(), &row, &session, Trigger::Close).is_none());
    assert!(engine.transcripts().is_empty());
    let ev = cx.store().unwrap().events(row.id, 100).unwrap();
    let last = ev.last().unwrap();
    assert_eq!(last.kind, "transcript_skipped");
    assert_eq!(last.payload.get("trigger"), Some(&json!("close")));
    assert_eq!(
        last.payload.get("reason"),
        Some(&json!("no agent session registered"))
    );
}

#[test]
fn a_session_unregistered_but_known_to_the_box_is_archived() {
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let (dir, cx) = fx(FakeEngine::new());
    let cx = cx.with_engine(engine.clone());
    let (row, session) = unregistered(&cx, dir.path());
    let store = cx.store().unwrap();
    let set = Fields {
        agent_session_id: Field::Set("agent-1".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(row.id).set(set)).unwrap();
    let row = store.box_row(row.id).unwrap().unwrap();
    assert!(save(&cx, store, &row, &session, Trigger::Close).is_some());
    assert_eq!(engine.transcripts().len(), 1);
}
