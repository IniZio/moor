use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec, SyncMode};
use moor_core::seam::proc::{Deadline, Output, Tool};

use crate::adapters::crabbox::Crabbox;
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
    let c = Crabbox::new(p.clone());
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

#[test]
fn streamed_run_reports_nonzero_exit() {
    let mut s = spec(SyncMode::Full);
    s.output = Some(Output::Inherit);
    let reply = Reply::Fail {
        code: 3,
        stdout: String::new(),
        stderr: String::new(),
    };
    let (out, _) = run_with(&s, reply);
    assert_eq!(out, Ok(ExecOutcome::Exited { code: 3 }));
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
