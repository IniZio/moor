#![allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    reason = "test assertions and skip notices"
)]

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{BlockKind, Life, Note, Probe, Question, Status, WaitCheck};
use moor_core::name::NameError;
use moor_core::seam::agent_adapter::{AgentAdapter, BoxSetup, EnvVar, Key, LaunchCtx, PromptMatch};
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::engine::{Engine, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{parse, script};
use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::hc::{self, Core};
use crate::testing::surface::FakeSurface;

struct Stub;

impl AgentAdapter for Stub {
    fn parse_note(&self, _: &serde_json::Value) -> Note {
        Note::default()
    }

    fn wait_check_script(&self) -> Option<String> {
        Some("check".to_owned())
    }

    fn first_run_prompt(&self, _: &str) -> Option<PromptMatch> {
        None
    }

    fn argv(&self, _: &LaunchCtx) -> Vec<String> {
        Vec::new()
    }

    fn resume_argv(&self, _: &str, _: &LaunchCtx) -> Vec<String> {
        Vec::new()
    }

    fn env(&self, _: &LaunchCtx) -> Vec<EnvVar> {
        Vec::new()
    }

    fn preseed(&self, _: &LaunchCtx) -> Result<BoxSetup, NameError> {
        Ok(BoxSetup {
            files: Vec::new(),
            step: String::new(),
        })
    }

    fn hook_writer(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
        self.preseed(ctx)
    }

    fn busy_marker(&self) -> &'static str {
        "busy"
    }

    fn menu_keys(&self, _: u8) -> Vec<Key> {
        Vec::new()
    }

    fn interrupt_key(&self) -> Key {
        Key::Escape
    }

    fn transcript_script(&self, _: &str) -> String {
        String::new()
    }
}

const DEADLINE: Deadline = Deadline(Duration::from_secs(3));

fn session() -> SessionRef {
    SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        pane: Some(PaneRef::new("p-1")),
    }
}

fn live_driver(driver: FakeDriver) -> Arc<FakeDriver> {
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
    BoxAgent::new(
        d.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(Stub),
        Arc::new(FakeClock::new(moor_core::seam::clock::Ts::from_millis(0))),
    )
}

/// Runs the probe script as a Box would, against `core`.
fn read(core: &Core) -> Probe {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new("sh")
        .args(["-c", &script()])
        .env("PATH", core.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    parse(&Claude, &String::from_utf8_lossy(&out.stdout))
}

/// The OSC 7501 report `fields` as the agent prints it, its message encoded as hc reads it.
fn osc(fields: &str, msg: &str) -> String {
    let encoded = String::from_utf8(
        Command::new("sh")
            .args(["-c", "printf %s \"$1\" | base64 | tr -d '\\n'", "b", msg])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let msg = if msg.is_empty() {
        String::new()
    } else {
        format!(":msg={encoded}")
    };
    format!("printf '\\033]7501;{fields}{msg}\\033\\\\'; sleep 30")
}

/// Starts the agent terminal on `core` and waits for it to report a status.
fn start_reporting(core: &Core, script: &str) {
    core.hc(&[
        "run",
        "--bg",
        "--no-parent",
        "--name",
        "moor-agent",
        "sh",
        "-c",
        script,
    ]);
    core.wait_get("moor-agent", "\"status\":{");
}

fn set_note(core: &Core, value: &serde_json::Value) {
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "doc.set",
        "params": { "name": "moor-agent", "value": value, "version": 0 }
    });
    let out = Command::new("sh")
        .args([
            "-c",
            "printf '%s\\n' \"$1\" | \"$2\" connect",
            "doc",
            &call.to_string(),
        ])
        .arg(core.bin().join("herdr-core"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"version\""),
        "{out:?}"
    );
}

#[test]
fn a_working_agent_reads_with_what_it_does_and_no_screen() {
    let Some(core) = hc::core() else { return };
    start_reporting(&core, &osc("state=working:app=claude-code", "Bash ls"));
    let got = read(&core);
    let term = got.term().unwrap();
    assert_eq!(term.life, Life::Live);
    assert_eq!(
        term.status,
        Some(Status::Working {
            doing: Some("Bash ls".to_owned())
        })
    );
    assert!(term.status_at.is_some());
    assert_eq!(got.screen(), None);
}

#[test]
fn a_blocked_question_reads_with_its_message_and_the_screen() {
    let Some(core) = hc::core() else { return };
    start_reporting(
        &core,
        &osc("state=blocked:app=claude-code:kind=question", "Pick one"),
    );
    let got = read(&core);
    assert_eq!(
        got.term().unwrap().status,
        Some(Status::Blocked {
            kind: Some(BlockKind::Question),
            msg: Some("Pick one".to_owned()),
        })
    );
    assert!(got.screen().is_some());
}

#[test]
fn idle_and_done_both_read_idle() {
    for state in ["idle", "done"] {
        let Some(core) = hc::core() else { return };
        start_reporting(&core, &osc(&format!("state={state}:app=claude-code"), ""));
        let got = read(&core);
        assert_eq!(got.term().unwrap().status, Some(Status::Idle), "{state}");
        assert!(got.screen().is_some(), "{state}");
    }
}

#[test]
fn a_terminal_that_ended_reads_finished_with_its_exit_code() {
    let Some(core) = hc::core() else { return };
    core.hc(&[
        "run",
        "--bg",
        "--no-parent",
        "--name",
        "moor-agent",
        "sh",
        "-c",
        "exit 3",
    ]);
    core.wait_get("moor-agent", "\"state\":\"finished\"");
    let term = read(&core).term().cloned().unwrap();
    assert_eq!(term.life, Life::Finished { exit: Some(3) });
}

#[test]
fn a_box_with_no_agent_terminal_reads_none() {
    let Some(core) = hc::core() else { return };
    core.hc(&[
        "run",
        "--bg",
        "--no-parent",
        "--name",
        "other",
        "sh",
        "-c",
        "sleep 30",
    ]);
    let got = read(&core);
    assert!(matches!(got, Probe::Read { term: None, .. }), "{got:?}");
}

#[test]
fn the_hooks_note_shows_up_beside_the_terminal() {
    let Some(core) = hc::core() else { return };
    start_reporting(&core, &osc("state=idle:app=claude-code", ""));
    set_note(
        &core,
        &serde_json::json!({
            "session_id": "sess-1",
            "said": "All done",
            "questions": [{"question": "Which?", "options": [{"label": "a"}, {"label": "b"}]}],
        }),
    );
    let note = read(&core).note().cloned().unwrap();
    assert_eq!(note.session_id.as_deref(), Some("sess-1"));
    assert_eq!(note.said.as_deref(), Some("All done"));
    assert_eq!(
        note.question,
        Some(Question {
            text: "Which?".to_owned(),
            options: vec!["a".to_owned(), "b".to_owned()],
        })
    );
}

#[test]
fn a_box_with_no_note_reads_an_empty_one() {
    let Some(core) = hc::core() else { return };
    start_reporting(&core, &osc("state=idle:app=claude-code", ""));
    assert_eq!(read(&core).note(), Some(&Note::default()));
}

#[test]
fn output_that_is_not_a_terminal_line_reads_timed_out() {
    assert_eq!(parse(&Claude, ""), Probe::TimedOut);
    assert_eq!(parse(&Claude, "not json\nnull\n"), Probe::TimedOut);
}

#[test]
fn a_failed_batch_reads_timed_out() {
    let d = Arc::new(FakeDriver::new());
    assert_eq!(agent(&d).probe(&session(), DEADLINE), Probe::TimedOut);
}

#[test]
fn the_read_is_bounded_by_the_probe_deadline() {
    let d = live_driver(FakeDriver::new().run_stdout("null\nnull\n"));
    let short = Deadline(Duration::from_millis(700));
    agent(&d).probe(&session(), short);
    assert_eq!(d.batch_deadlines(), [short]);
}

#[test]
fn a_working_read_touches_the_box_lease_and_an_idle_one_does_not() {
    let working = "{\"state\":\"live\",\"status\":{\"state\":\"working\"},\"turn\":0}\nnull\n";
    let idle = "{\"state\":\"live\",\"status\":{\"state\":\"idle\"},\"turn\":1}\nnull\n";
    let d = live_driver(FakeDriver::new().run_stdouts([working, idle]));
    agent(&d).probe(&session(), DEADLINE);
    assert_eq!(d.touches(), [BoxHandle::new("box-1")]);
    agent(&d).probe(&session(), DEADLINE);
    assert_eq!(d.touches().len(), 1);
}

#[test]
fn a_wait_check_reads_only_a_plain_answer() {
    for (stdout, want) in [
        ("none\n", WaitCheck::Nothing),
        ("running\n", WaitCheck::Running),
        ("unknown\n", WaitCheck::Unknown),
        ("", WaitCheck::Unknown),
        ("none of it\n", WaitCheck::Unknown),
    ] {
        let d = live_driver(FakeDriver::new().run_stdout(stdout));
        let got = agent(&d).check_wait(&session(), DEADLINE);
        assert_eq!(got, want, "{stdout:?}");
    }
}

#[test]
fn a_box_that_is_gone_answers_no_wait_check() {
    let d = Arc::new(FakeDriver::new().run_stdout("none\n"));
    let got = agent(&d).check_wait(&session(), DEADLINE);
    assert_eq!(got, WaitCheck::Unknown);
}
