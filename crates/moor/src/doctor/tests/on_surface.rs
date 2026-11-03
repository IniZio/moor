//! The surface check.

use super::*;

#[test]
fn herdr_answering_is_ok() {
    let rig = Rig::new();
    rig.proc.on(
        Tool::Herdr,
        &["workspace", "list"],
        Reply::Ok("{}".to_owned()),
    );
    let c = rig.with(surface::check);
    assert_eq!(c.status, Status::Ok, "{c:?}");
    assert!(c.detail.contains("session moor"));
}

#[test]
fn missing_herdr_fails_with_the_config_fix() {
    let c = Rig::new().with(surface::check);
    assert_eq!(c.status, Status::Fail);
    assert!(c.fix.contains("surface.kind"));
}

#[test]
fn herdr_without_a_server_warns_because_the_controller_starts_it() {
    let rig = Rig::new();
    rig.proc.on(
        Tool::Herdr,
        &["workspace", "list"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: r#"{"error":{"code":"server_not_running","message":"x"}}"#.to_owned(),
        },
    );
    assert_eq!(rig.with(surface::check).status, Status::Warn);
}

#[test]
fn pty_needs_the_feature() {
    let rig = Rig::new().with_config("surface:\n  kind: pty\n");
    let want = if cfg!(feature = "pty-surface") {
        Status::Ok
    } else {
        Status::Fail
    };
    assert_eq!(rig.with(surface::check).status, want);
}
