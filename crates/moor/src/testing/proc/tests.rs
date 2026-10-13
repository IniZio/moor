use std::time::Duration;

use moor_core::seam::proc::{Deadline, Output};

use super::*;

fn spec(tool: Tool, args: &[&str]) -> ProcSpec {
    ProcSpec::new(tool, Deadline(Duration::from_secs(2))).args(args.iter().copied())
}

#[test]
fn first_match_wins_and_rules_reuse() {
    let p = ScriptedProc::new();
    p.on(Tool::Git, &["status"], Reply::Ok("a".into()))
        .on(Tool::Git, &[], Reply::Ok("b".into()));
    for _ in 0..2 {
        assert_eq!(
            p.run(&spec(Tool::Git, &["status", "-s"])).unwrap().stdout,
            "a"
        );
    }
    assert_eq!(p.run(&spec(Tool::Git, &["log"])).unwrap().stdout, "b");
    assert_eq!(p.calls().len(), 3);
}

#[test]
fn unscripted_is_127() {
    let p = ScriptedProc::new();
    let e = p.run(&spec(Tool::Gh, &["pr"])).unwrap_err();
    assert!(
        matches!(e, ProcError::NonZero { code: 127, ref stderr_tail, .. } if stderr_tail == "unscripted: gh pr")
    );
}

#[test]
fn fail_and_timeout() {
    let p = ScriptedProc::new();
    p.on(
        Tool::Ssh,
        &["a"],
        Reply::Fail {
            code: 2,
            stdout: "o".into(),
            stderr: "e".into(),
        },
    )
    .on(Tool::Ssh, &["b"], Reply::Timeout);
    assert!(matches!(
        p.run(&spec(Tool::Ssh, &["a"])),
        Err(ProcError::NonZero { code: 2, .. })
    ));
    assert!(matches!(
        p.run(&spec(Tool::Ssh, &["b"])),
        Err(ProcError::Timeout { after, .. }) if after == Duration::from_secs(2)
    ));
}

#[test]
fn detached_records_and_ok() {
    let p = ScriptedProc::new();
    p.spawn_detached(&spec(Tool::MoorSelf, &["proxy"]).env("K", "V").cwd("/w"))
        .unwrap();
    let calls = p.calls();
    let c = calls.first().unwrap();
    assert_eq!(c.env, [("K".to_owned(), "V".to_owned())]);
    assert_eq!(
        (c.tool, c.args.as_slice()),
        (Tool::MoorSelf, ["proxy".to_owned()].as_slice())
    );
    assert_eq!(c.cwd.as_deref(), Some(std::path::Path::new("/w")));
}

#[test]
fn bytes_reply_is_raw_for_secret_and_lossy_for_run() {
    let p = ScriptedProc::new();
    p.on(Tool::Gh, &["s"], Reply::Bytes(vec![0xff, 0xfe, b'x']));
    let out = p.run_secret(&spec(Tool::Gh, &["s"])).unwrap();
    assert_eq!(out.as_slice(), [0xff, 0xfe, b'x']);
    let lossy = p.run(&spec(Tool::Gh, &["s"])).unwrap();
    assert_eq!(lossy.stdout, "\u{fffd}\u{fffd}x");
    assert_eq!(p.calls().len(), 2);
}

#[test]
fn secret_failure_has_empty_stdout() {
    let p = ScriptedProc::new();
    p.on(
        Tool::Gh,
        &["f"],
        Reply::Fail {
            code: 4,
            stdout: "leak".into(),
            stderr: "e".into(),
        },
    );
    assert!(matches!(
        p.run_secret(&spec(Tool::Gh, &["f"])),
        Err(ProcError::NonZero { code: 4, stdout, .. }) if stdout.is_empty()
    ));
}

#[test]
fn streamed_reports_nonzero_as_an_error() {
    let p = ScriptedProc::new();
    p.on(Tool::Sh, &["ok"], Reply::Ok(String::new())).on(
        Tool::Sh,
        &["bad"],
        Reply::Fail {
            code: 3,
            stdout: "out".into(),
            stderr: "boom".into(),
        },
    );
    assert!(p
        .run_streamed(&spec(Tool::Sh, &["ok"]), &Output::Inherit)
        .is_ok());
    assert!(matches!(
        p.run_streamed(&spec(Tool::Sh, &["bad"]), &Output::Inherit),
        Err(ProcError::NonZero { code: 3, stdout, stderr_tail })
            if stdout.is_empty() && stderr_tail == "boom"
    ));
    assert_eq!(p.calls().len(), 2);
}

#[test]
fn streamed_signal_and_interrupt() {
    let p = ScriptedProc::new();
    p.on(Tool::Sh, &["sig"], Reply::Signalled(9))
        .on(Tool::Sh, &["int"], Reply::Interrupted);
    assert!(matches!(
        p.run_streamed(&spec(Tool::Sh, &["sig"]), &Output::Inherit),
        Err(ProcError::Signalled(9))
    ));
    assert!(matches!(
        p.run_streamed(&spec(Tool::Sh, &["int"]), &Output::Log("/dev/null".into())),
        Err(ProcError::Interrupted)
    ));
}

fn child_spec(args: &[&str]) -> ProcSpec {
    spec(Tool::Crabbox, args).env("K", "V").cwd("/w")
}

#[test]
fn child_records_call_and_ready_replays_line() {
    let p = ScriptedProc::new();
    p.on_child(Tool::Crabbox, &["tunnel"], ChildReply::Ready("up".into()));
    let mut c = p
        .spawn_child(&child_spec(&["tunnel", "--reverse"]))
        .unwrap();
    let d = Deadline(Duration::from_secs(5));
    assert_eq!(c.first_line(d).unwrap(), "up");
    assert_eq!(c.first_line(d).unwrap(), "up");
    let calls = p.calls();
    let call = calls.first().unwrap();
    assert_eq!(call.tool, Tool::Crabbox);
    assert_eq!(call.args, ["tunnel", "--reverse"]);
    assert_eq!(call.env, [("K".to_owned(), "V".to_owned())]);
    assert_eq!(call.cwd.as_deref(), Some(std::path::Path::new("/w")));
    assert_eq!(call.deadline, Deadline(Duration::from_secs(2)));
}

#[test]
fn child_pids_are_distinct() {
    let p = ScriptedProc::new();
    p.on_child(Tool::Crabbox, &[], ChildReply::Hang);
    let a = p.spawn_child(&child_spec(&["a"])).unwrap();
    let b = p.spawn_child(&child_spec(&["b"])).unwrap();
    assert_ne!(a.pid(), b.pid());
}

#[test]
fn child_exit_reports_code_and_stderr() {
    let p = ScriptedProc::new();
    p.on_child(
        Tool::Crabbox,
        &["tunnel"],
        ChildReply::Exit {
            code: 3,
            stderr: "boom".into(),
        },
    );
    let mut c = p.spawn_child(&child_spec(&["tunnel"])).unwrap();
    assert!(matches!(
        c.first_line(Deadline(Duration::from_secs(1))),
        Err(FirstLineError::Exited { code: Some(3), signal: None, stderr_tail })
            if stderr_tail == "boom"
    ));
}

#[test]
fn child_hang_times_out_with_passed_deadline() {
    let p = ScriptedProc::new();
    p.on_child(Tool::Crabbox, &["tunnel"], ChildReply::Hang);
    let mut c = p.spawn_child(&child_spec(&["tunnel"])).unwrap();
    assert!(matches!(
        c.first_line(Deadline(Duration::from_millis(7))),
        Err(FirstLineError::Timeout { tool: Tool::Crabbox, after })
            if after == Duration::from_millis(7)
    ));
}

#[test]
fn unscripted_child_is_spawn_error() {
    let p = ScriptedProc::new();
    let e = p.spawn_child(&child_spec(&["tunnel"])).err().unwrap();
    assert!(matches!(
        e,
        ProcError::Spawn(ref io)
            if io.kind() == std::io::ErrorKind::NotFound
                && io.to_string() == "unscripted: crabbox tunnel"
    ));
    assert_eq!(p.calls().len(), 1);
}

#[test]
fn dropping_child_is_recorded() {
    let p = ScriptedProc::new();
    p.on_child(Tool::Crabbox, &[], ChildReply::Ready("x".into()));
    let c = p.spawn_child(&child_spec(&["t"])).unwrap();
    let pid = c.pid();
    assert!(p.dropped().is_empty());
    assert!(p.alive(pid));
    drop(c);
    assert_eq!(p.dropped(), [pid]);
    assert!(!p.alive(pid));
}
