use std::sync::Arc;

use moor_core::controller::FrameErrorKind;
use moor_core::domain::decision::{Decision, ExtraEvent, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::BoxName;

use super::*;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;

#[test]
fn prints_stored_events_then_caught_up_and_returns() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let new = NewBox {
        name: BoxName::parse_allow_bare("a").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(new)).unwrap().task;
    for i in 0..2 {
        let event = ExtraEvent::Copied {
            from: format!("a{i}"),
            to: format!("b{i}"),
        };
        store.apply(Decision::new(id).event(event)).unwrap();
    }
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().unwrap()),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let mut out = Vec::new();
    watch_local(&env, 0, &WatchFilter::default(), false, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let copied = text.lines().filter(|l| l.contains(r#""kind":"copied""#));
    assert_eq!(copied.count(), 2, "{text}");
    let last = text.lines().last().unwrap();
    assert!(last.contains(r#""frame":"caught-up""#), "{text}");
}

#[test]
fn error_frame_is_an_error_and_not_printed() {
    let error = ServerFrame::Error {
        id: None,
        kind: FrameErrorKind::SlowClient,
        message: "too slow".to_owned(),
    };
    let mut out = Vec::new();
    let mut sink = Sink {
        out: &mut out,
        failure: None,
    };
    sink.write_all(error.to_line().as_bytes()).unwrap();
    let failure = sink.failure.take().unwrap();
    assert!(failure.contains("too slow"), "{failure}");
    assert!(out.is_empty());
}
