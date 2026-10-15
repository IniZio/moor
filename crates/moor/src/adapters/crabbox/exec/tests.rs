use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::name::BoxHandle;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{Driver, DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output, Tool};

use crate::adapters::crabbox::Crabbox;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Call, Reply, ScriptedProc};

fn spec(sync: SyncMode) -> ExecSpec {
    ExecSpec {
        argv: vec!["true".to_owned()],
        env: Vec::new(),
        cwd: None,
        sync,
        output: None,
        host_cwd: None,
        class: None,
    }
}

fn run_with(
    spec: &ExecSpec,
    reply: Reply,
) -> (
    Result<ExecOutcome, moor_core::seam::driver::DriverError>,
    Call,
) {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["run"], reply);
    let c = Crabbox::fake(p.clone());
    let out = c.exec(
        &BoxHandle::new("sprites/moor-t7-ab12cd"),
        spec,
        Deadline(Duration::from_secs(90)),
    );
    let call = p.calls().into_iter().next().unwrap();
    (out, call)
}

fn ok() -> Reply {
    Reply::Ok(String::new())
}

fn has(call: &Call, flag: &str) -> bool {
    call.args.iter().any(|a| a == flag)
}

fn class_of(call: &Call) -> Option<&str> {
    let pos = call.args.iter().position(|a| a == "--class")?;
    call.args.get(pos + 1).map(String::as_str)
}

#[test]
fn never_hydrates_in_either_sync_mode() {
    for sync in [SyncMode::Full, SyncMode::None] {
        let (_, call) = run_with(&spec(sync), ok());
        assert!(has(&call, "--no-hydrate"));
    }
}

#[test]
fn no_sync_flag_only_for_sync_none() {
    let (_, none) = run_with(&spec(SyncMode::None), ok());
    let (_, full) = run_with(&spec(SyncMode::Full), ok());
    assert!(has(&none, "--no-sync"));
    assert!(!has(&full, "--no-sync"));
}

#[test]
fn class_maps_to_crabbox_class() {
    for (size, class) in [("small", "standard"), ("large", "large")] {
        let mut s = spec(SyncMode::Full);
        s.class = Some(size.to_owned());
        let (_, call) = run_with(&s, ok());
        assert_eq!(class_of(&call), Some(class));
    }
}

#[test]
fn no_class_flag_without_a_class() {
    let (_, call) = run_with(&spec(SyncMode::Full), ok());
    assert!(!has(&call, "--class"));
}

#[test]
fn git_seed_source_is_local_for_crabbox_itself() {
    let (_, call) = run_with(&spec(SyncMode::Full), ok());
    assert!(call
        .env
        .contains(&("CRABBOX_SYNC_GIT_SEED_SOURCE".into(), "local".into())));
    assert!(!has(&call, "--allow-env"));
}

#[test]
fn host_cwd_is_the_proc_cwd() {
    let mut s = spec(SyncMode::Full);
    s.host_cwd = Some(PathBuf::from("/work/tree"));
    let (_, call) = run_with(&s, ok());
    assert_eq!(call.cwd, Some(PathBuf::from("/work/tree")));
}

fn streamed(reply: impl Fn() -> Reply) -> Vec<Result<ExecOutcome, DriverError>> {
    [Output::Inherit, Output::Log("/dev/null".into())]
        .into_iter()
        .map(|output| {
            let mut s = spec(SyncMode::Full);
            s.output = Some(output);
            run_with(&s, reply()).0
        })
        .collect()
}

fn fail(code: i32, stderr: &str) -> Reply {
    Reply::Fail {
        code,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn streamed_remote_exit_is_exited() {
    for out in streamed(|| fail(3, "sync done\nremote command exited 3\n")) {
        assert_eq!(out, Ok(ExecOutcome::Exited { code: 3 }));
    }
}

#[test]
fn streamed_success_is_exit_zero() {
    for out in streamed(ok) {
        assert_eq!(out, Ok(ExecOutcome::Exited { code: 0 }));
    }
}

#[test]
fn streamed_crabbox_config_failure_is_an_error() {
    for out in streamed(|| fail(3, "bad config")) {
        assert!(
            matches!(&out, Err(DriverError::Failed { summary }) if summary.contains("configuration problem")),
            "{out:?}"
        );
    }
}

#[test]
fn streamed_box_not_found_is_not_found() {
    for out in streamed(|| fail(4, "Error: box not found")) {
        assert_eq!(out, Err(DriverError::NotFound));
    }
}

#[test]
fn streamed_sync_failure_and_lost_box_are_errors() {
    for out in streamed(|| fail(5, "box lost during warmup")) {
        assert!(
            matches!(&out, Err(DriverError::Failed { summary }) if summary.contains("box lost during warmup")),
            "{out:?}"
        );
    }
    for out in streamed(|| fail(1, "rsync failed")) {
        assert!(matches!(out, Err(DriverError::Failed { .. })), "{out:?}");
    }
}

#[test]
fn interrupt_and_signal_are_interrupted() {
    for output in [None, Some(Output::Inherit)] {
        for reply in [Reply::Interrupted, Reply::Signalled(15)] {
            let mut s = spec(SyncMode::Full);
            s.output = output.clone();
            let (out, _) = run_with(&s, reply);
            assert_eq!(out, Ok(ExecOutcome::Interrupted));
        }
    }
}

const RACE: &str = "lease cbx_1 claim changed; retry";

fn race() -> Reply {
    Reply::Fail {
        code: 2,
        stdout: String::new(),
        stderr: RACE.to_owned(),
    }
}

fn run_calls(p: &ScriptedProc) -> usize {
    p.calls()
        .iter()
        .filter(|c| c.args.first().is_some_and(|a| a == "run"))
        .count()
}

fn exec_once(c: &Crabbox) -> Result<ExecOutcome, DriverError> {
    c.exec(
        &BoxHandle::new("sprites/moor-t7-ab12cd"),
        &spec(SyncMode::None),
        Deadline(Duration::from_secs(90)),
    )
}

#[test]
fn claim_race_is_retried_until_the_call_succeeds() {
    let p = Arc::new(ScriptedProc::new());
    p.once(Tool::Crabbox, &["run"], race());
    p.on(Tool::Crabbox, &["run"], ok());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let c = Crabbox::new(p.clone(), clock.clone());
    assert_eq!(exec_once(&c), Ok(ExecOutcome::Exited { code: 0 }));
    assert_eq!(run_calls(&p), 2);
    assert_eq!(clock.now(), Ts::from_millis(300));
}

#[test]
fn claim_race_gives_up_after_three_attempts() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["run"], race());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let c = Crabbox::new(p.clone(), clock.clone());
    let err = exec_once(&c).unwrap_err();
    assert_eq!(run_calls(&p), 3);
    assert_eq!(clock.now(), Ts::from_millis(900));
    assert_eq!(err.to_string(), "the Box lease was busy; try again");
}

#[test]
fn other_failures_are_not_retried() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: "claim changed\nremote command exited 2\n".to_owned(),
        },
    );
    let c = Crabbox::fake(p.clone());
    assert_eq!(exec_once(&c), Ok(ExecOutcome::Exited { code: 2 }));
    assert_eq!(run_calls(&p), 1);
}

#[test]
fn cd_wrapper_enters_directory_and_keeps_arguments() {
    let out = std::process::Command::new("sh")
        .args([
            "-c",
            super::CD_THEN_EXEC,
            "sh",
            "/",
            "sh",
            "-c",
            r#"pwd; printf '[%s]' "$1" "$2""#,
            "sh",
            "a b  c",
            "it's \"q\" $HOME",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("/\n"), "{text}");
    assert!(text.ends_with(r#"[a b  c][it's "q" $HOME]"#), "{text}");
}
