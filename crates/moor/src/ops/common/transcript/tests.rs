use std::fs;
use std::os::unix::fs::PermissionsExt;

use moor_core::decide::transcript::Trigger;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::engine::EngineError;
use moor_core::{ProviderId, TaskName};
use serde_json::json;

use super::save;
use crate::cx::Cx;
use crate::store::{SessionRow, TaskRow};
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
        Arc::new(FakeSurface::new("/s")),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(Arc::new(engine));
    (dir, cx)
}

fn ready(cx: &Cx, dir: &std::path::Path) -> (TaskRow, SessionRow) {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("t").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        worktree: Field::Set(dir.display().to_string()),
        box_workdir: Field::Set("/work/box-1".into()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: moor_core::BoxHandle::new("box-1"),
        });
    store.apply(d).unwrap();
    store.begin_session(id, "s-1", Some("p-1")).unwrap();
    (
        store.task(id).unwrap().unwrap(),
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
        .join(format!("transcripts/{}/{}-1", row.id.get(), session.id));
    assert_eq!(saved.path, want.display().to_string());
    assert_eq!(
        saved.digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        fs::metadata(&want).unwrap().permissions().mode() & 0o777,
        0o600
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
    assert!(again.path.ends_with(&format!("{}-2", session.id)));
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
