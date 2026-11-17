//! Tests for `retry`.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::engine::{LaunchPrompt, SessionRef};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, BoxReq, ErrorKind, StartReq};

use super::run;
use crate::cx::Cx;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;

mod bring_up;

struct World {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
}

fn name() -> BoxName {
    BoxName::parse_allow_bare("demo").unwrap()
}

const fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// A world whose forge serves a repo with `.moor.yaml` text `yaml`.
fn world(yaml: &str, driver: FakeDriver) -> World {
    let dir = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=4).fold(driver, |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
    let forge = Arc::new(FakeForge::new());
    forge.answer(ForgeRepo::new("o", "n"), Ok(repo_at("abc123", Some(yaml))));
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let cx = Cx::fake_with_clock(
        dir.path(),
        driver.clone(),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
        clock,
    )
    .with_forge(forge);
    World {
        _dir: dir,
        cx,
        driver,
    }
}

/// Starts `demo` with no agent, whether or not the start succeeds, and returns its id.
fn started(w: &World) -> BoxId {
    started_with(w, None)
}

/// The request `started_with` sends, with no prompt.
fn start_req() -> StartReq {
    StartReq {
        name: name(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: true,
        resume: false,
        repo: Some(ForgeRepo::new("o", "n")),
        parent: None,
        also: None,
        identity: None,
    }
}

/// Starts `demo` with no agent but `prompt` stored, and returns its id.
fn started_with(w: &World, prompt: Option<&str>) -> BoxId {
    let req = StartReq {
        name: name(),
        prompt: prompt.map(str::to_owned),
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: true,
        resume: false,
        repo: Some(ForgeRepo::new("o", "n")),
        parent: None,
        also: None,
        identity: None,
    };
    let _ = crate::ops::start::run(&w.cx, &req);
    w.cx.store()
        .unwrap()
        .open_box_by_name(&name())
        .unwrap()
        .unwrap()
        .id
}

fn req() -> BoxReq {
    BoxReq { box_name: name() }
}

fn row(w: &World, id: BoxId) -> crate::store::BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

#[test]
fn retry_after_a_failed_clone_runs_the_steps_again_and_clears_the_failure() {
    let w = world(
        "setup: [a]\n",
        FakeDriver::new().exec_results([Ok(exit(1))]),
    );
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    assert!(row(&w, id).setup_failed_at.is_some());
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.attached);
    assert!(v.setup.ran && v.setup.ok);
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(after.setup_failed_at.is_none());
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn retry_in_the_same_box_delivers_the_egress_forwarder_again() {
    let w = world(
        "setup: [a]\n",
        FakeDriver::new().exec_results([Ok(exit(1))]),
    );
    let id = started(&w);
    let store = w.cx.store().unwrap();
    let _ = store
        .reserve(id, &crate::testing::rng::FakeRng::default())
        .unwrap();
    assert!(store.set_egress_token(id, b"t1").unwrap());
    assert!(store.note_egress_token_delivered(id, b"t1", 1).unwrap());
    run(&w.cx, &req()).unwrap();
    assert!(!store.egress_token_delivered(id, 1).unwrap());
}

#[test]
fn retry_of_a_box_that_did_not_fail_setup_is_refused() {
    let w = world("setup: [a]\n", FakeDriver::new());
    let id = started(&w);
    let before = row(&w, id).lifecycle;
    assert_ne!(before, BoxLifecycle::SetupFailed);
    let e = run(&w.cx, &req()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(
        e.error.to_string().starts_with("not_setup_failed: "),
        "{}",
        e.error
    );
    assert_eq!(row(&w, id).lifecycle, before);
}

#[test]
fn retry_of_an_unknown_box_says_there_is_none() {
    let w = world("setup: [a]\n", FakeDriver::new());
    let e = run(&w.cx, &req()).unwrap_err();
    assert!(e.error.to_string().contains("no open Box demo"));
}

#[test]
fn retry_where_the_machine_is_gone_acquires_a_new_one_and_runs_setup() {
    let w = world(
        "setup: [a]\n",
        FakeDriver::new().exec_results([Ok(exit(1))]),
    );
    let id = started(&w);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
    let acquires = w.driver.calls().iter().filter(|c| *c == "acquire").count();
    assert_eq!(acquires, 2);
}

/// A launched session.
fn pane_session() -> SessionRef {
    SessionRef {
        id: String::new(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
    }
}

/// A world with a failing first setup step whose engine answers a launch with a session.
fn world_with_engine(driver: FakeDriver) -> (World, Arc<FakeEngine>) {
    yaml_world_with_engine("setup: [a]\n", driver)
}

/// A world serving `.moor.yaml` text `yaml` whose engine answers a launch with a session.
fn yaml_world_with_engine(yaml: &str, driver: FakeDriver) -> (World, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(pane_session())));
    let mut w = world(yaml, driver);
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

#[test]
fn a_successful_retry_launches_the_agent_with_the_stored_prompt() {
    let (w, engine) = world_with_engine(FakeDriver::new().exec_results([Ok(exit(1))]));
    let id = started_with(&w, Some("fix it"));
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    assert!(engine.launches().is_empty());
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.agent.launched);
    let launches = engine.launches();
    assert_eq!(launches.len(), 1);
    let prompts: Vec<_> = launches.iter().map(|l| l.prompt.clone()).collect();
    assert_eq!(prompts, [LaunchPrompt::Text("fix it".into())]);
    assert!(w.cx.store().unwrap().live_launch(id).unwrap().is_some());
}

#[test]
fn a_retry_whose_setup_fails_again_launches_no_agent() {
    let (w, engine) = world_with_engine(FakeDriver::new().exec_results([Ok(exit(1)), Ok(exit(1))]));
    let id = started_with(&w, Some("fix it"));
    let _ = run(&w.cx, &req());
    assert!(engine.launches().is_empty());
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
}

/// The setup execs, in order, as the Box they ran on and their script.
fn setup_execs(w: &World) -> Vec<(String, String)> {
    w.driver
        .execs()
        .into_iter()
        .filter(|(_, spec, _)| spec.argv.iter().any(|a| a.contains("moor setup: $")))
        .map(|(h, spec, _)| (h.as_str().to_owned(), spec.argv.join(" ")))
        .collect()
}

const TWO_LINES: &str = "setup: [prep-one, run-two]\n";

#[test]
fn setup_runs_as_one_script_and_a_retry_runs_it_all_again() {
    let w = world(TWO_LINES, FakeDriver::new().exec_results([Ok(exit(1))]));
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ok);
    let execs = setup_execs(&w);
    assert_eq!(execs.len(), 1, "{execs:?}");
    assert!(execs[0].1.contains("prep-one") && execs[0].1.contains("run-two"));
}

/// The `line` of Box `id`'s latest `setup_failed` event.
fn failed_line(w: &World, id: BoxId) -> Option<String> {
    let store = w.cx.store().unwrap();
    let event = store
        .newest_events_of(id, "setup_failed", 1)
        .unwrap()
        .pop()?;
    Some(event.payload.get("line")?.as_str()?.to_owned())
}

#[test]
fn a_failed_setup_line_leaves_the_box_setup_failed_and_names_the_line() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let w = world(TWO_LINES, driver.copy_text("moor setup: $ prep-one\n"));
    let id = started(&w);
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::SetupFailed);
    assert!(after.setup_failed_at.is_some() && after.effect.is_none());
    assert_eq!(failed_line(&w, id).as_deref(), Some("prep-one"));
}

#[test]
fn a_retry_after_a_failed_line_reruns_in_the_kept_machine_and_launches_the_agent() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, engine) = yaml_world_with_engine(TWO_LINES, driver);
    let id = started_with(&w, Some("fix it"));
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    assert!(engine.launches().is_empty());
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ok && v.agent.launched);
    let after = row(&w, id);
    let on = (after.lifecycle, after.hands.unwrap().handle);
    assert_eq!(on, (BoxLifecycle::Ready, Some(BoxHandle::new("box-1"))));
    let hosts: Vec<_> = setup_execs(&w).into_iter().map(|(h, _)| h).collect();
    assert_eq!(
        hosts,
        ["box-1", "box-1"],
        "setup ran again in the kept machine"
    );
    let calls = w.driver.calls();
    assert_eq!(calls.iter().filter(|c| *c == "acquire").count(), 1);
    let prompts: Vec<_> = engine.launches().into_iter().map(|l| l.prompt).collect();
    assert_eq!(prompts, [LaunchPrompt::Text("fix it".into())]);
}

/// The number of machines acquired so far.
fn acquires(w: &World) -> usize {
    w.driver.calls().iter().filter(|c| *c == "acquire").count()
}

/// A working Box whose machine is lost; the relaunch sets up on a new machine from scratch and
/// fails `run-two`.
fn relaunch_failing_a_line() -> (World, Arc<FakeEngine>, BoxId) {
    let (w, engine) = yaml_world_with_engine(TWO_LINES, FakeDriver::new());
    let id = started_with(&w, Some("fix it"));
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    w.driver.push_exec_result(Ok(exit(0)));
    w.driver.push_exec_result(Ok(exit(1)));
    let _ = crate::ops::start::run(&w.cx, &start_req());
    (w, engine, id)
}

#[test]
fn a_ready_box_whose_relaunch_fails_a_line_holds_in_setup_failed_with_no_agent() {
    let (w, engine, id) = relaunch_failing_a_line();
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::SetupFailed);
    assert!(after.effect.is_none());
    assert_eq!(failed_line(&w, id).as_deref(), Some("run-two"));
    assert_eq!(acquires(&w), 2, "one relaunch");
    assert!(engine.launches().is_empty(), "no agent on the new machine");
}

#[test]
fn a_retry_of_a_ready_box_whose_relaunch_failed_a_line_runs_the_lines_again_in_the_new_machine() {
    let (w, _engine, id) = relaunch_failing_a_line();
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ran && v.setup.ok);
    let ready = row(&w, id);
    let on = (ready.lifecycle, ready.hands.unwrap().handle);
    assert_eq!(on, (BoxLifecycle::Ready, Some(BoxHandle::new("box-2"))));
    assert_eq!(acquires(&w), 2, "the retry relaunches nothing");
    let on_new = setup_execs(&w).into_iter().filter(|(h, _)| h == "box-2");
    assert_eq!(
        on_new.count(),
        2,
        "the relaunch's failed run, then the retry's"
    );
}
