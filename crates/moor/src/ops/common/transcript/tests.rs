use std::fs;
use std::os::unix::fs::PermissionsExt;

use moor_core::decide::transcript::Trigger;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::EngineError;
use moor_core::{BoxName, ProviderId};
use serde_json::json;

use super::save;
use crate::blobs::fs::FsBlobs;
use crate::blobs::{BlobKey, Blobs};
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
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(Arc::new(engine));
    (dir, cx)
}

fn ready(cx: &Cx) -> (BoxRow, SessionRow) {
    let (row, session) = unregistered(cx);
    let store = cx.store().unwrap();
    store.set_registered_session(session.id, "agent-1").unwrap();
    let session = store.live_session(row.id).unwrap().unwrap();
    (row, session)
}

fn unregistered(cx: &Cx) -> (BoxRow, SessionRow) {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("t").unwrap(),
            repo: None,
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
    let (row, session) = ready(&cx);
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
    let (_dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (row, session) = ready(&cx);
    let store = cx.store().unwrap();
    save(&cx, store, &row, &session, Trigger::Relaunch).unwrap();
    let again = save(&cx, store, &row, &session, Trigger::Close).unwrap();
    assert!(again.path.ends_with(&format!("{}-2.tgz", session.id)));
}

#[test]
fn a_later_unchanged_save_keeps_the_earlier_archive() {
    let (_dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (row, mut session) = ready(&cx);
    let store = cx.store().unwrap();
    let first = save(&cx, store, &row, &session, Trigger::Turn).unwrap();
    store
        .set_transcript_digest(session.id, &first.digest)
        .unwrap();
    session.transcript_digest = Some(first.digest.clone());
    let again = save(&cx, store, &row, &session, Trigger::Close).unwrap();
    assert_eq!(again.path, first.path);
    assert_eq!(fs::read(&first.path).unwrap(), b"abc");
}

#[test]
fn a_turn_save_gives_up_sooner_than_the_last_save_of_a_session() {
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone());
    let (row, session) = ready(&cx);
    let store = cx.store().unwrap();
    for trigger in [
        Trigger::Turn,
        Trigger::Exit,
        Trigger::End,
        Trigger::Close,
        Trigger::Relaunch,
    ] {
        save(&cx, store, &row, &session, trigger);
    }
    let secs: Vec<u64> = engine
        .transcript_deadlines()
        .iter()
        .map(|d| d.0.as_secs())
        .collect();
    assert_eq!(secs, [15, 60, 60, 60, 60]);
}

#[test]
fn a_failure_records_the_error_and_saves_nothing() {
    let engine =
        FakeEngine::new().fail_transcript(EngineError::Transcript("archive: exit 3".into()));
    let (_dir, cx) = fx(engine);
    let (row, session) = ready(&cx);
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
    let (_dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let (mut row, session) = ready(&cx);
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
    let (_dir, cx) = fx(engine);
    let (row, session) = ready(&cx);
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
    let (_dir, cx) = fx(FakeEngine::new());
    let cx = cx.with_engine(engine.clone());
    let (row, session) = unregistered(&cx);
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
    let (_dir, cx) = fx(FakeEngine::new());
    let cx = cx.with_engine(engine.clone());
    let (row, session) = unregistered(&cx);
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

fn not_linked() -> EngineError {
    EngineError::Box(DriverError::Transport {
        stderr_tail: "machine 2 is not linked".into(),
    })
}

fn failed_events(cx: &Cx, row: &BoxRow) -> Vec<serde_json::Value> {
    let events = cx.store().unwrap().events(row.id, 100).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind == "transcript_failed")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_save_failing_for_want_of_a_link_is_recorded_as_pending() {
    let (_dir, cx) = fx(FakeEngine::new().fail_transcript(not_linked()));
    let (row, session) = ready(&cx);
    assert!(save(&cx, cx.store().unwrap(), &row, &session, Trigger::Turn).is_none());
    let failed = failed_events(&cx, &row);
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed.first().and_then(|f| f.get("unlinked")),
        Some(&json!(true))
    );
}

#[test]
fn a_save_failing_otherwise_is_not_marked_unlinked() {
    let engine =
        FakeEngine::new().fail_transcript(EngineError::Transcript("stream: bad base64".into()));
    let (_dir, cx) = fx(engine);
    let (row, session) = ready(&cx);
    assert!(save(&cx, cx.store().unwrap(), &row, &session, Trigger::Turn).is_none());
    assert_eq!(
        failed_events(&cx, &row)
            .first()
            .and_then(|f| f.get("unlinked")),
        Some(&json!(false))
    );
}

fn round_trip(blobs: Arc<dyn Blobs>) {
    let (_dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let cx = cx.with_blobs(blobs);
    let (row, session) = ready(&cx);
    let store = cx.store().unwrap();
    let saved = save(&cx, store, &row, &session, Trigger::Close).unwrap();
    let key = BlobKey::transcript(row.id, session.id, 1);
    assert_eq!(cx.blobs().unwrap().get(&key).unwrap(), b"abc");
    assert_eq!(saved.path, cx.blobs().unwrap().locate(&key));
    let staged = cx.state_dir().unwrap().join("transcripts/.staging");
    assert_eq!(fs::read_dir(staged).unwrap().count(), 0);
}

#[test]
fn a_transcript_round_trips_through_the_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    round_trip(Arc::new(FsBlobs::new(dir.path())));
}

#[test]
#[ignore = "needs MOOR_TEST_S3_ENDPOINT"]
fn s3_transcript_round_trip() {
    round_trip(crate::blobs::tests::s3());
}

#[test]
fn an_unreachable_blob_store_records_the_failure_and_stops_nothing() {
    let (_dir, cx) = fx(FakeEngine::new().transcript_text("abc"));
    let cx = cx.with_blobs(crate::blobs::tests::unreachable());
    let (row, session) = ready(&cx);
    let store = cx.store().unwrap();
    assert!(save(&cx, store, &row, &session, Trigger::Close).is_none());
    let events = store.events(row.id, usize::MAX).unwrap();
    let failed = events
        .iter()
        .find(|e| e.kind == "transcript_failed")
        .unwrap();
    let error = failed.payload.get("error").unwrap().as_str().unwrap();
    assert!(error.contains("blob store http"), "{error}");
}
