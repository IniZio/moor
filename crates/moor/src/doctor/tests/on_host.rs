//! The host checks: the forwarder.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::*;

fn mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn rig_with(pairs: &[(&str, &str)]) -> Rig {
    let mut rig = Rig::new();
    rig.env = Env::from_pairs(pairs.iter().copied());
    rig
}

#[test]
fn egress_forward_passes_when_the_resolved_binary_is_executable() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("moor-egress-forward");
    fs::write(&bin, "#!/bin/sh\n").unwrap();
    mode(&bin, 0o755);
    let state = dir.path().join("state");
    let rig = rig_with(&[
        ("MOOR_STATE_DIR", state.to_str().unwrap()),
        ("MOOR_EGRESS_FORWARD_BIN", bin.to_str().unwrap()),
    ]);
    let c = rig.with(host::egress);
    assert_eq!(c.status, Status::Ok, "{c:?}");
    assert!(c.detail.contains("moor-egress-forward"), "{c:?}");
}

#[test]
fn egress_forward_fails_when_the_binary_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let missing = dir.path().join("nope");
    let rig = rig_with(&[
        ("MOOR_STATE_DIR", state.to_str().unwrap()),
        ("MOOR_EGRESS_FORWARD_BIN", missing.to_str().unwrap()),
    ]);
    let c = rig.with(host::egress);
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(c.detail.contains("nope"), "{c:?}");
    assert!(c.fix.contains("MOOR_EGRESS_FORWARD_BIN"), "{c:?}");
}

#[test]
fn egress_forward_is_not_needed_when_boxes_reach_a_remote_proxy() {
    let rig = rig_with(&[
        ("MOOR_STATE_DIR", "/s"),
        ("MOOR_EGRESS_URL", "wss://egress.test/egress"),
    ]);
    let c = rig.with(host::egress);
    assert_eq!(c.status, Status::Ok, "{c:?}");
}
