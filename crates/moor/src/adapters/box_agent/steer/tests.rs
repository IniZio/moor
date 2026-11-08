#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index what they just checked"
)]

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::driver::FakeDriver;
use crate::testing::hc::{self, Core};

const DEADLINE: Deadline = Deadline(Duration::from_secs(3));

fn session() -> SessionRef {
    SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
    }
}

fn live(driver: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(driver);
    d.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    d
}

fn agent(d: &Arc<FakeDriver>) -> BoxAgent {
    BoxAgent::new(d.clone(), Arc::new(Claude))
}

fn text(t: &str) -> Steer {
    Steer::Text(t.to_owned())
}

/// The one Box command a steer runs, after the driver answered with `d`'s scripted results.
fn command_of(d: &Arc<FakeDriver>) -> String {
    let execs = d.execs();
    assert_eq!(execs.len(), 1, "one command per steer");
    let (_, spec, _) = execs.first().unwrap();
    spec.argv.last().unwrap().clone()
}

/// The command that steering the agent with `what` runs on a Box.
fn command(what: &Steer) -> String {
    let d = live(FakeDriver::new());
    agent(&d).steer(&session(), what, DEADLINE).unwrap();
    command_of(&d)
}

/// Runs a Box command against `core`, as the Box's shell would.
fn on_box(core: &Core, script: &str) -> (i32, String) {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new("sh")
        .args(["-c", script])
        .env("PATH", core.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Starts the agent terminal on `core` running `shell`.
fn start_agent(core: &Core, shell: &str) {
    core.hc(&[
        "run",
        "--bg",
        "--no-parent",
        "--name",
        "moor-agent",
        "sh",
        "-c",
        shell,
    ]);
}

/// Waits up to five seconds for the agent terminal's screen to contain `needle`.
fn wait_screen(core: &Core, needle: &str) {
    use moor_core::seam::clock::Clock as _;
    let clock = crate::adapters::clock::SystemClock::real();
    let until = clock.instant() + Duration::from_secs(5);
    while !core.hc(&["read", "moor-agent"]).contains(needle) {
        assert!(clock.instant() < until, "the screen never showed {needle}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn text_is_sent_through_herdr_core_in_one_command() {
    let d = live(FakeDriver::new().run_stdout(r#"{"started":true}"#));
    let out = agent(&d)
        .steer(&session(), &text("it's done"), DEADLINE)
        .unwrap();
    assert_eq!(
        out,
        SteerOutcome {
            submit_retried: false,
            submit_checked: true
        }
    );
    let script = command_of(&d);
    assert!(
        script.ends_with("herdr-core send --force --json moor-agent 'it'\\''s done'"),
        "{script}"
    );
    assert_eq!(d.batch_connects().len(), 1);
}

#[test]
fn text_hc_did_not_start_is_not_a_checked_submit() {
    for stdout in [r#"{"started":false}"#, "", "not json"] {
        let d = live(FakeDriver::new().run_stdout(stdout));
        let out = agent(&d).steer(&session(), &text("go"), DEADLINE).unwrap();
        assert_eq!(
            out,
            SteerOutcome {
                submit_retried: false,
                submit_checked: false
            },
            "{stdout:?}"
        );
    }
}

#[test]
fn an_interrupt_sends_the_profile_key_and_types_nothing() {
    let d = live(FakeDriver::new().run_stdout(r#"{"started":true}"#));
    let out = agent(&d)
        .steer(&session(), &Steer::Interrupt, DEADLINE)
        .unwrap();
    assert_eq!(
        out,
        SteerOutcome {
            submit_retried: false,
            submit_checked: false
        }
    );
    assert!(command_of(&d).ends_with("herdr-core keys moor-agent esc"));
}

#[test]
fn menu_sends_the_choice_key() {
    assert!(command(&Steer::Menu(2)).ends_with("herdr-core keys moor-agent 2"));
}

#[test]
fn a_terminal_hc_cannot_find_or_that_ended_is_agent_gone() {
    for code in [3, 11] {
        let d = live(FakeDriver::new());
        for what in [text("go"), Steer::Menu(1), Steer::Interrupt] {
            d.push_exec_result(Ok(ExecOutcome::Exited { code }));
            let out = agent(&d).steer(&session(), &what, DEADLINE);
            assert_eq!(out, Err(EngineError::AgentGone), "exit {code}: {what:?}");
        }
    }
}

#[test]
fn any_other_failure_fails_the_step_it_was_sending() {
    let cases = [
        (text("go"), "type"),
        (Steer::Menu(1), "menu"),
        (Steer::Interrupt, "interrupt"),
    ];
    for (what, step) in cases {
        let d = live(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]));
        let out = agent(&d).steer(&session(), &what, DEADLINE);
        assert!(
            matches!(&out, Err(EngineError::Failed { step: s, .. }) if *s == step),
            "{out:?}"
        );
    }
}

#[test]
fn an_unreachable_box_is_a_box_error() {
    let d = live(FakeDriver::new());
    d.push_exec_result(Err(DriverError::NotFound));
    let out = agent(&d).steer(&session(), &text("go"), DEADLINE);
    assert_eq!(out, Err(EngineError::Box(DriverError::NotFound)));
}

#[test]
fn text_reaches_the_agent_terminal() {
    let Some(core) = hc::core() else { return };
    start_agent(&core, "exec cat");
    core.wait_get("moor-agent", "\"state\":\"live\"");
    let (status, stdout) = on_box(&core, &command(&text("hello from mcp")));
    assert_eq!(status, 0);
    assert!(stdout.contains("\"started\""), "{stdout}");
    wait_screen(&core, "hello from mcp");
}

#[test]
fn a_menu_key_and_escape_reach_the_agent_terminal() {
    let Some(core) = hc::core() else { return };
    start_agent(&core, "stty raw -echo; echo ready; exec cat -v");
    wait_screen(&core, "ready");
    assert_eq!(on_box(&core, &command(&Steer::Menu(2))).0, 0);
    wait_screen(&core, "2");
    assert_eq!(on_box(&core, &command(&Steer::Interrupt)).0, 0);
    wait_screen(&core, "^[");
}

#[test]
fn a_finished_terminal_answers_with_the_status_that_is_agent_gone() {
    let Some(core) = hc::core() else { return };
    start_agent(&core, "exit 0");
    core.wait_get("moor-agent", "\"state\":\"finished\"");
    for what in [text("go"), Steer::Menu(1), Steer::Interrupt] {
        assert_eq!(on_box(&core, &command(&what)).0, 11, "{what:?}");
    }
}

#[test]
fn a_box_with_no_agent_terminal_answers_with_the_status_that_is_agent_gone() {
    let Some(core) = hc::core() else { return };
    assert_eq!(on_box(&core, &command(&text("go"))).0, 3);
}
