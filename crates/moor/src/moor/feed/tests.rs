//! Tests for the controller-facing event and settle facade.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::controller::ServerFrame;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxId, BoxName, ProviderId};

use super::Moor;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// Context over `driver` with state under `dir` and a scripted engine.
fn engine_cx(dir: &Path, driver: &Arc<FakeDriver>, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Adds a Working Box `name` with a live machine and an open session `session`.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let store = cx.store().unwrap();
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    store.begin_session(id, session, None).unwrap();
    id
}

/// Probes whose hook says the agent is idle, written just before the fake clock's now.
fn idle_hook(cx: &Cx) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(cx.clock().now().as_millis() - 1000),
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

/// A Working Box whose fake engine reports quiet, with the context that settles it.
fn quiet_agent(dir: &Path) -> Cx {
    let driver = Arc::new(FakeDriver::new().exec_stdout([""]));
    let cx = engine_cx(dir, &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", "s-a");
    let engine = Arc::new(FakeEngine::new().probes("s-a", idle_hook(&cx)));
    engine_cx(dir, &driver, &engine)
}

#[test]
fn each_event_frame_carries_its_box_id_when_names_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let old = working(&cx, &driver, "alpha", "s1");
    let close = Decision::new(old)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    cx.store().unwrap().apply(close).unwrap();
    let newer = working(&cx, &driver, "alpha", "s2");
    let store = cx.store().unwrap();
    let mut want: Vec<(i64, BoxId)> = [old, newer]
        .iter()
        .flat_map(|id| {
            store
                .events(*id, 100)
                .unwrap()
                .into_iter()
                .map(|e| (e.id, *id))
        })
        .collect();
    want.sort_unstable();
    let got: Vec<(i64, BoxId)> = Moor::from_cx(cx)
        .events_after(0, 100)
        .unwrap()
        .into_iter()
        .map(|f| match f {
            ServerFrame::Event {
                cursor,
                r#box,
                box_id,
                ..
            } => {
                assert_eq!(r#box.as_deref(), Some("alpha"));
                (cursor, box_id.unwrap())
            }
            other => unreachable!("not an event: {other:?}"),
        })
        .collect();
    assert_eq!(got, want);
    assert!(want.iter().any(|(_, id)| *id == old) && want.iter().any(|(_, id)| *id == newer));
}

#[test]
fn events_after_maps_rows_to_frames_strictly_after_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "alpha", "s1");
    let moor = Moor::from_cx(cx);
    let all = moor.events_after(0, 100).unwrap();
    assert!(all.len() >= 2, "{all:?}");
    let first = match all.first() {
        Some(ServerFrame::Event { cursor, r#box, .. }) => {
            assert_eq!(r#box.as_deref(), Some("alpha"));
            *cursor
        }
        other => unreachable!("not an event: {other:?}"),
    };
    let later = moor.events_after(first, 100).unwrap();
    assert_eq!(later.len(), all.len() - 1);
    assert!(later
        .iter()
        .all(|f| matches!(f, ServerFrame::Event { cursor, .. } if *cursor > first)));
    assert_eq!(moor.events_after(0, 1).unwrap().len(), 1);
}

#[test]
fn settle_open_stores_an_idled_event_for_a_quiet_agent() {
    let dir = tempfile::tempdir().unwrap();
    let moor = Moor::from_cx(quiet_agent(dir.path()));
    let msgs = moor.settle_open(&|| false);
    assert!(msgs.is_empty(), "{msgs:?}");
    let frames = moor.events_after(0, 100).unwrap();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Event { kind, .. } if kind == "idled")),
        "{frames:?}"
    );
}
