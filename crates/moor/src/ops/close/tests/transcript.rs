use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::seam::engine::EngineError;

use super::{fx_with, req, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::surface::FakeSurface;

const SHA_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn with_engine(engine: &Arc<FakeEngine>, surface: FakeSurface) -> Fx {
    let f = fx_with(FakeDriver::new(), surface, 0);
    Fx {
        cx: f.cx.with_engine(engine.clone()),
        ..f
    }
}

fn kinds(f: &Fx) -> Vec<String> {
    let ev = f.cx.store().unwrap().events(f.id, 100).unwrap();
    ev.into_iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("transcript_"))
        .collect()
}

fn ended_digest(f: &Fx) -> Option<String> {
    let c = rusqlite::Connection::open(f.dir.path().join("moor.db")).unwrap();
    c.query_row(
        "SELECT transcript_digest FROM sessions WHERE task = ?",
        [f.id.get()],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn close_saves_before_stopping_the_agent_and_records_it() {
    let surface = Arc::new(FakeSurface::new("/s"));
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let stopped_first = Arc::new(AtomicBool::new(false));
    {
        let (surface, flag) = (surface.clone(), stopped_first.clone());
        engine.on_transcript(move || {
            let closed = surface.calls().iter().any(|c| c.starts_with("close_pane"));
            flag.store(closed, Ordering::SeqCst);
        });
    }
    let f = fx_with(FakeDriver::new(), FakeSurface::new("/s"), 0);
    let f = Fx {
        cx: f.cx.with_engine(engine.clone()),
        surface,
        ..f
    };
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-1"))
        .unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(engine.transcripts(), ["s-1"]);
    assert!(!stopped_first.load(Ordering::SeqCst));
    let path = v.transcript.unwrap();
    assert!(path.contains("/transcripts/"), "{path}");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(kinds(&f), ["transcript_saved"]);
    assert_eq!(ended_digest(&f).as_deref(), Some(SHA_ABC));
}

#[test]
fn a_failed_save_records_the_failure_and_close_still_succeeds() {
    let engine = Arc::new(
        FakeEngine::new().fail_transcript(EngineError::Transcript("archive: exit 3".into())),
    );
    let f = with_engine(&engine, FakeSurface::new("/s"));
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-1"))
        .unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.agent_stopped && v.box_deleted);
    assert_eq!(v.transcript, None);
    assert_eq!(kinds(&f), ["transcript_failed"]);
    assert_eq!(ended_digest(&f), None);
}

#[test]
fn no_session_means_no_save() {
    let engine = Arc::new(FakeEngine::new().transcript_text("abc"));
    let f = with_engine(&engine, FakeSurface::new("/s"));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.transcript, None);
    assert!(engine.transcripts().is_empty());
    assert!(kinds(&f).is_empty());
}
