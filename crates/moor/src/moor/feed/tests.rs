//! Tests for the controller-facing event and settle facade.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::config::DEFAULT_PROOF_MAX_BYTES;
use moor_core::controller::ServerFrame;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
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
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Adds a Working Task `name` with a live Box and an open session `session`.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    let store = cx.store().unwrap();
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            worktree: Field::Set("/wt".into()),
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

/// A Working Task whose fake engine reports quiet, with the context that settles it.
fn quiet_agent(dir: &Path) -> Cx {
    let driver = Arc::new(FakeDriver::new().exec_stdout([""]));
    let cx = engine_cx(dir, &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", "s-a");
    let engine = Arc::new(FakeEngine::new().probes("s-a", idle_hook(&cx)));
    engine_cx(dir, &driver, &engine)
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
        Some(ServerFrame::Event { cursor, task, .. }) => {
            assert_eq!(task, "alpha");
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
    let msgs = moor.settle_open(DEFAULT_PROOF_MAX_BYTES);
    assert!(msgs.is_empty(), "{msgs:?}");
    let frames = moor.events_after(0, 100).unwrap();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Event { kind, .. } if kind == "idled")),
        "{frames:?}"
    );
}

#[test]
fn settle_open_records_and_copies_a_new_proof_file() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .exec_stdout([
                "shot.png\t4\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            ])
            .copy_text("png!"),
    );
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "t", "s-1");
    let moor = Moor::from_cx(cx);
    let msgs = moor.settle_open(DEFAULT_PROOF_MAX_BYTES);
    assert!(msgs.is_empty(), "{msgs:?}");
    let frames = moor.events_after(0, 100).unwrap();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Event { kind, task, .. } if kind == "proof" && task.as_str() == "t")),
        "{frames:?}"
    );
    assert_eq!(id.get(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("proof/1/shot.png")).unwrap(),
        "png!"
    );
}
