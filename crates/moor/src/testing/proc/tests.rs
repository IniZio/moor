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
fn streamed_passes_exit_codes_through() {
    let p = ScriptedProc::new();
    p.on(Tool::Sh, &["ok"], Reply::Ok(String::new())).on(
        Tool::Sh,
        &["bad"],
        Reply::Fail {
            code: 3,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    assert_eq!(
        p.run_streamed(&spec(Tool::Sh, &["ok"]), &Output::Inherit)
            .unwrap(),
        0
    );
    assert_eq!(
        p.run_streamed(&spec(Tool::Sh, &["bad"]), &Output::Inherit)
            .unwrap(),
        3
    );
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
