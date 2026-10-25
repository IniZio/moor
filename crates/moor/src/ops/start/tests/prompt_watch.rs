use std::sync::Arc;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::seam::agent_profile::FirstRunPrompt;
use moor_core::seam::clock::Ts;
use moor_core::StartReq;

use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::{FakeEngine, THIS_LAUNCH};

/// The agent session the first Box's launch records.
const SESSION: &str = "moor-1";

/// Pane reads of a watch that runs its whole window.
const FULL_WINDOW: usize = 17;

fn goal_req(name: &str) -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req(name)
    }
}

/// A hook file read showing `kind`.
fn hook(kind: HookKind) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(1_000),
        }),
        pane: PaneRead::Absent,
    }
}

/// The starting marker this launch writes.
fn marker() -> Probes {
    hook(HookKind::Starting {
        launch: Some(THIS_LAUNCH.to_owned()),
    })
}

/// An Engine whose agent runs no hook while a first-run screen is up.
fn quiet() -> FakeEngine {
    FakeEngine::new().hooks_quiet_during_first_run()
}

/// A world whose engine knows the folder-trust prompt and whose pane shows `tail`.
fn watching(engine: FakeEngine, tail: &str) -> World {
    let engine = Arc::new(engine.prompt("trust this folder", FirstRunPrompt::FolderTrust));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine);
    w.surface.push_pane_obs(
        "p-1",
        Ok(PaneObs {
            status: PaneStatus::Unknown,
            tail: tail.to_owned(),
            prompt: None,
            exit_marker: None,
        }),
    );
    w
}

/// How many times the agent pane was read.
fn pane_reads(w: &World) -> usize {
    w.surface
        .calls()
        .iter()
        .filter(|c| *c == "pane_obs p-1")
        .count()
}

#[test]
fn a_hook_report_after_the_launch_marker_ends_the_watch_early() {
    let engine = quiet()
        .then_probes(SESSION, marker())
        .then_probes(SESSION, marker())
        .probes(SESSION, hook(HookKind::Busy));
    let w = watching(engine, "working");
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(pane_reads(&w), 3);
}

#[test]
fn any_report_but_the_launch_marker_counts_as_past_startup() {
    let engine = quiet()
        .then_probes(SESSION, marker())
        .probes(SESSION, hook(HookKind::Idle));
    let w = watching(engine, "working");
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(pane_reads(&w), 2);
}

#[test]
fn a_report_never_preceded_by_the_launch_marker_keeps_the_full_window() {
    let engine = quiet().probes(SESSION, hook(HookKind::Busy));
    let w = watching(engine, "working");
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(pane_reads(&w), FULL_WINDOW);
}

#[test]
fn only_the_launch_marker_keeps_the_full_window() {
    let engine = quiet().probes(SESSION, marker());
    let w = watching(engine, "working");
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(pane_reads(&w), FULL_WINDOW);
}

#[test]
fn a_prompt_on_screen_is_recorded_before_a_report_ends_the_watch() {
    let engine = quiet()
        .then_probes(SESSION, marker())
        .probes(SESSION, hook(HookKind::Busy));
    let w = watching(engine, "working");
    w.surface.push_pane_obs(
        "p-1",
        Ok(PaneObs {
            status: PaneStatus::Unknown,
            tail: "Do you trust this folder?".to_owned(),
            prompt: None,
            exit_marker: None,
        }),
    );
    let v = run(&w.cx, &goal_req("demo")).unwrap();
    let blocked =
        w.cx.store()
            .unwrap()
            .events(moor_core::BoxId::new(v.boxed.id), 50)
            .unwrap()
            .into_iter()
            .any(|e| e.kind == "agent_prompt_blocked");
    assert!(blocked);
    assert_eq!(pane_reads(&w), 2);
}

#[test]
fn a_stale_marker_then_a_foreign_report_keeps_the_full_window() {
    let engine = quiet()
        .then_probes(
            SESSION,
            hook(HookKind::Starting {
                launch: Some("0ld000".to_owned()),
            }),
        )
        .probes(SESSION, hook(HookKind::Busy));
    let w = watching(engine, "working");
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(pane_reads(&w), FULL_WINDOW);
}

#[test]
fn an_agent_that_may_report_during_first_run_keeps_the_full_window() {
    let engine = FakeEngine::new()
        .then_probes(SESSION, marker())
        .probes(SESSION, hook(HookKind::Busy));
    let w = watching(engine, "working");
    run(&w.cx, &goal_req("demo")).unwrap();
    assert_eq!(pane_reads(&w), FULL_WINDOW);
}
