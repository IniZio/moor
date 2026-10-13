use std::sync::Arc;

use moor_core::activity::PaneStatus;
use moor_core::name::TaskName;
use moor_core::seam::clock::Ts;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::{PaneRef, ResumeOutcome, ResumeReg, Surface, SurfaceError};

use crate::adapters::herdr::Herdr;
use crate::testing::proc::{Reply, ScriptedProc};

fn fixture() -> (Arc<ScriptedProc>, Herdr) {
    let p = Arc::new(ScriptedProc::new());
    let h = Herdr::new(p.clone(), "sess".to_owned(), Vec::new());
    (p, h)
}

fn reg() -> ResumeReg {
    ResumeReg {
        agent: "claude".to_owned(),
        state: PaneStatus::Idle,
        session_id: "abc-123".to_owned(),
        task: TaskName::parse("a").expect("valid name"),
    }
}

fn refusal(code: &str) -> Reply {
    Reply::Fail {
        code: 1,
        stdout: format!(r#"{{"error":{{"code":"{code}","message":"no"}}}}"#),
        stderr: String::new(),
    }
}

fn seq_of(args: &[String]) -> u64 {
    let i = args.iter().position(|a| a == "--seq").expect("has --seq");
    args.get(i + 1)
        .and_then(|v| v.parse().ok())
        .expect("numeric seq")
}

#[test]
fn reports_the_agent_then_the_resume_command_with_bare_moor() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["pane"], Reply::Ok(String::new()));
    let got = h.register_resume(&PaneRef::new("w1:p2"), &reg());
    assert_eq!(got, Ok(ResumeOutcome::Registered));
    let calls = p.calls();
    assert_eq!(calls.len(), 2);
    let (Some(first), Some(second)) = (calls.first(), calls.get(1)) else {
        return;
    };
    let head = [
        "pane",
        "report-agent",
        "w1:p2",
        "--source",
        "moor",
        "--agent",
        "claude",
        "--state",
        "idle",
    ];
    assert!(first.args.starts_with(&head.map(String::from)));
    assert!(second
        .args
        .starts_with(&["pane".to_owned(), "report-agent-session".to_owned()]));
    assert!(second.args.ends_with(&[
        "--".to_owned(),
        "moor".to_owned(),
        "start".to_owned(),
        "a".to_owned(),
        "--resume".to_owned()
    ]));
    assert!(second
        .args
        .windows(2)
        .any(|w| w == ["--agent-session-id", "abc-123"]));
}

#[test]
fn seq_rises_on_every_call_and_across_registrations() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["pane"], Reply::Ok(String::new()));
    h.register_resume(&PaneRef::new("p"), &reg()).unwrap();
    h.register_resume(&PaneRef::new("p"), &reg()).unwrap();
    let seqs: Vec<u64> = p.calls().iter().map(|c| seq_of(&c.args)).collect();
    assert_eq!(seqs.len(), 4);
    assert!(seqs.is_sorted_by(|a, b| a < b), "{seqs:?}");
}
#[test]
fn seq_starts_above_the_clock_seed() {
    let (p, h) = fixture();
    let h = h.seq_from(Ts::from_millis(5));
    p.on(Tool::Herdr, &["pane"], Reply::Ok(String::new()));
    h.register_resume(&PaneRef::new("p"), &reg()).unwrap();
    assert!(p.calls().iter().all(|c| seq_of(&c.args) > 5000));
}

#[test]
fn a_declined_resume_is_rejected_not_failed() {
    for code in ["invalid_resume_argv", "resume_not_accepted"] {
        let (p, h) = fixture();
        p.on(
            Tool::Herdr,
            &["pane", "report-agent"],
            Reply::Ok(String::new()),
        );
        p.on(
            Tool::Herdr,
            &["pane", "report-agent-session"],
            refusal(code),
        );
        assert_eq!(
            h.register_resume(&PaneRef::new("p"), &reg()),
            Ok(ResumeOutcome::Rejected(code.to_owned()))
        );
    }
}

#[test]
fn a_missing_pane_is_an_error() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["pane"], refusal("pane_not_found"));
    assert_eq!(
        h.register_resume(&PaneRef::new("p"), &reg()),
        Err(SurfaceError::NotFound)
    );
}
