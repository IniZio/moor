//! A clone or agent install that fails while a Box comes up holds it in setup-failed for a retry.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::agent_adapter::AgentInstall;
use moor_core::seam::engine::LaunchPrompt;
use moor_core::BoxId;

use super::{
    exit, failed_line, pane_session, req, row, run, start_req, started_with, world, World,
};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const INSTALL: &str = "echo install-the-agent";

const CURL: &str = "curl: (92) HTTP/2 stream 1 was not closed cleanly: INTERNAL_ERROR (err 2)";

/// A world over `setup: [a]` whose engine installs its agent with [`INSTALL`], and whose install
/// log ends with curl's error.
fn install_world(driver: FakeDriver) -> (World, Arc<FakeEngine>) {
    let engine = FakeEngine::new()
        .launch_result(Ok(pane_session()))
        .install(AgentInstall {
            script: INSTALL.to_owned(),
            dirs: Vec::new(),
            check: "true".to_owned(),
        });
    let engine = Arc::new(engine);
    let mut w = world("setup: [a]\n", driver);
    w.cx = w.cx.with_engine(engine.clone());
    let log = w.cx.state_dir().unwrap().join("agent-install/1.log");
    w.driver.on_exec(move || {
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, format!("{CURL}\n")).unwrap();
    });
    (w, engine)
}

/// Loses the machine of working Box `id` and starts it again.
fn relaunched(w: &World, id: BoxId) {
    assert_eq!(row(w, id).lifecycle, BoxLifecycle::Ready);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let _ = crate::ops::start::run(&w.cx, &start_req());
}

/// The install execs so far.
fn installs(w: &World) -> usize {
    let execs = w.driver.execs();
    execs
        .iter()
        .filter(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(INSTALL)))
        .count()
}

/// Asserts Box `id` holds in setup-failed with `cause` in its event, then retries it and asserts
/// the retry runs the install again and launches the agent with the stored prompt.
fn holds_then_resumes(w: &World, engine: &FakeEngine, id: BoxId, cause: &str) {
    assert_eq!(row(w, id).lifecycle, BoxLifecycle::SetupFailed);
    let line = failed_line(w, id).unwrap_or_default();
    assert!(line.contains(cause), "{line}");
    assert!(engine.launches().is_empty(), "no agent on the failed Box");
    let before = installs(w);
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ok && v.agent.launched);
    assert_eq!(row(w, id).lifecycle, BoxLifecycle::Ready);
    assert_eq!(installs(w), before + 1, "the retry installs again");
    let prompts: Vec<_> = engine.launches().into_iter().map(|l| l.prompt).collect();
    assert_eq!(prompts, [LaunchPrompt::Text("fix it".into())]);
}

#[test]
fn a_failed_install_on_a_first_start_holds_in_setup_failed_with_the_installer_error() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, engine) = install_world(driver);
    let id = started_with(&w, Some("fix it"));
    holds_then_resumes(&w, &engine, id, CURL);
}

#[test]
fn a_failed_install_on_a_relaunch_holds_in_setup_failed_with_the_installer_error() {
    let driver = FakeDriver::new().exec_results([
        Ok(exit(0)),
        Ok(exit(0)),
        Ok(exit(0)),
        Ok(exit(0)),
        Ok(exit(0)),
        Ok(exit(1)),
    ]);
    let (w, engine) = install_world(driver);
    let id = started_with(&w, Some("fix it"));
    relaunched(&w, id);
    holds_then_resumes(&w, &engine, id, CURL);
}

#[test]
fn a_failed_clone_on_a_relaunch_holds_in_setup_failed_and_a_retry_resumes_it() {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, engine) = install_world(driver);
    let id = started_with(&w, Some("fix it"));
    relaunched(&w, id);
    holds_then_resumes(&w, &engine, id, "cloning the repo into the Box failed");
}
