use std::time::Duration;

use moor_core::seam::proc::Deadline;

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
