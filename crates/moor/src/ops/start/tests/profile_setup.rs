//! The profile's own `setup.sh` runs once per new Box, after the fetch, and never fails the start.

use std::time::Duration;

use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::proc::Output;

use super::profile::{answering, set_profile, world_answering, HEAD};
use super::relaunch::{gone, session};
use super::setup::warning_codes;
use super::*;
use crate::testing::engine::FakeEngine;

const MARK: &str = "sh setup.sh";

struct SetupExec {
    argv: Vec<String>,
    env: Vec<(String, String)>,
    output: Option<Output>,
    deadline: Duration,
}

fn setup_execs(w: &World) -> Vec<SetupExec> {
    w.driver
        .execs()
        .into_iter()
        .filter(|(_, spec, _)| spec.argv.join(" ").contains(MARK))
        .map(|(_, spec, deadline)| SetupExec {
            argv: spec.argv,
            env: spec.env,
            output: spec.output,
            deadline: deadline.0,
        })
        .collect()
}

fn profile_failures(w: &World, id: BoxId) -> Vec<(String, String)> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind == "profile_failed")
        .map(|e| {
            let text = |k: &str| {
                e.payload
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_owned()
            };
            (text("step"), text("detail"))
        })
        .collect()
}

fn started(w: &World) -> BoxId {
    BoxId::new(run(&w.cx, &req("demo")).unwrap().boxed.id)
}

#[test]
fn setup_sh_runs_from_the_checkout_with_a_log_and_the_ten_minute_deadline() {
    let w = world_answering(None, HEAD, None);
    set_profile(&w, None);
    let id = started(&w);
    let got = setup_execs(&w);
    assert_eq!(got.len(), 1);
    let SetupExec {
        argv,
        env,
        output,
        deadline,
    } = got.into_iter().next().unwrap();
    let script = argv.last().unwrap();
    assert!(script.contains("cd \"$d\""), "{script}");
    assert!(script.contains("$HOME/.moor-profile/person"), "{script}");
    assert_eq!(deadline, Duration::from_mins(10));
    let log = w.dir.path().join("logs").join(id.get().to_string());
    assert_eq!(output, Some(Output::Log(log.join("profile-setup.log"))));
    let fetch_env = w
        .driver
        .execs()
        .into_iter()
        .find(|(_, s, _)| s.argv.join(" ").contains("git fetch"));
    assert_eq!(Some(env), fetch_env.map(|(_, s, _)| s.env));
    assert!(profile_failures(&w, id).is_empty());
}

#[test]
fn a_failing_setup_sh_is_reported_and_the_box_still_starts() {
    let failing = FakeDriver::new().exec_ends_when(MARK, ExecOutcome::Exited { code: 3 });
    let w = world_answering_with(failing);
    w.cx.take_warnings();
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    let failures = profile_failures(&w, BoxId::new(v.boxed.id));
    assert_eq!(failures, [("setup".to_owned(), "exit 3".to_owned())]);
    assert!(warning_codes(&w).contains(&"profile_failed".to_owned()));
}

#[test]
fn an_interrupted_setup_sh_is_reported() {
    let driver = FakeDriver::new().exec_ends_when(MARK, ExecOutcome::Interrupted);
    let w = world_answering_with(driver);
    let id = started(&w);
    assert_eq!(
        profile_failures(&w, id),
        [("setup".to_owned(), "interrupted".to_owned())]
    );
}

fn world_answering_with(driver: FakeDriver) -> World {
    let w = answering(driver, None, HEAD, None);
    set_profile(&w, None);
    w
}

#[test]
fn a_box_without_a_profile_runs_no_setup_sh() {
    let w = world(FakeDriver::new());
    started(&w);
    assert!(setup_execs(&w).is_empty());
}

#[test]
fn a_failed_fetch_runs_no_setup_sh() {
    let driver = FakeDriver::new().exec_ends_when("git fetch", ExecOutcome::Exited { code: 128 });
    let w = world_answering_with(driver);
    let id = started(&w);
    assert!(setup_execs(&w).is_empty());
    let steps: Vec<String> = profile_failures(&w, id).into_iter().map(|f| f.0).collect();
    assert_eq!(steps, ["fetch"]);
}

#[test]
fn a_relaunch_does_not_run_setup_sh_again() {
    let engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .launch_result(Ok(session("p-2")));
    let mut w = world_answering(None, HEAD, None);
    w.cx = w.cx.with_engine(Arc::new(engine));
    set_profile(&w, None);
    started(&w);
    assert_eq!(setup_execs(&w).len(), 1);
    gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(setup_execs(&w).len(), 1);
}

#[test]
fn the_profile_records_one_step_around_its_fetch_and_setup() {
    let w = world_answering(None, HEAD, None);
    set_profile(&w, None);
    let id = started(&w);
    let kinds: Vec<_> = super::agent_install::step_events(&w, id)
        .into_iter()
        .map(|e| e.1)
        .collect();
    assert_eq!(
        kinds,
        [
            serde_json::json!({"step": "profile"}),
            serde_json::json!({"step": "profile", "ms": 0, "ok": true}),
        ]
    );
}

#[test]
fn a_failing_setup_sh_ends_the_profile_step_not_ok() {
    let driver = FakeDriver::new().exec_ends_when(MARK, ExecOutcome::Exited { code: 3 });
    let w = world_answering_with(driver);
    let id = started(&w);
    let last = super::agent_install::step_events(&w, id).pop().unwrap();
    assert_eq!(
        last.1,
        serde_json::json!({"step": "profile", "ms": 0, "ok": false})
    );
}

#[test]
fn a_box_without_a_profile_records_no_profile_step() {
    let w = world(FakeDriver::new());
    let id = started(&w);
    assert!(super::agent_install::step_events(&w, id).is_empty());
}
