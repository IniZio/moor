//! The host checks: the forwarder, `/dev/kvm` and the crabbox config file.

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

#[test]
fn kvm_passes_when_the_device_opens_for_read_and_write() {
    let dir = tempfile::tempdir().unwrap();
    let dev = dir.path().join("kvm");
    fs::write(&dev, "").unwrap();
    let c = host::kvm_at(&dev);
    assert_eq!(c.status, Status::Ok, "{c:?}");
}

#[test]
fn kvm_fails_with_the_group_fix_when_the_device_is_not_writable() {
    let dir = tempfile::tempdir().unwrap();
    let dev = dir.path().join("kvm");
    fs::write(&dev, "").unwrap();
    mode(&dev, 0o000);
    let c = host::kvm_at(&dev);
    if rustix::process::geteuid().is_root() {
        return;
    }
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(c.fix.contains("kvm group"), "{c:?}");
}

#[test]
fn kvm_fails_when_the_device_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let c = host::kvm_at(&dir.path().join("kvm"));
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(c.detail.contains("not found"), "{c:?}");
}

#[test]
fn kvm_is_checked_only_for_a_local_node_that_runs_cloudhypervisor() {
    let none = Rig::new().with_config("nodes:\n  n:\n    runtime: static\n");
    assert!(none.with(host::checks).iter().all(|c| c.name != "kvm"));
    let some = Rig::new().with_config("nodes:\n  n:\n    runtime: cloudhypervisor\n");
    assert!(some.with(host::checks).iter().any(|c| c.name == "kvm"));
}

fn config_in(dir: &Path, file_mode: u32) -> Rig {
    let cfg = dir.join("crabbox");
    fs::create_dir_all(&cfg).unwrap();
    mode(dir, 0o755);
    mode(&cfg, 0o755);
    let file = cfg.join("config.yaml");
    fs::write(&file, "provider: external\n").unwrap();
    mode(&file, file_mode);
    rig_with(&[("XDG_CONFIG_HOME", dir.to_str().unwrap())])
}

#[test]
fn crabbox_config_passes_at_0600_in_a_private_directory() {
    let dir = tempfile::tempdir().unwrap();
    let rig = config_in(dir.path(), 0o600);
    let c = rig.with(host::crabbox_config);
    assert_eq!(c.status, Status::Ok, "{c:?}");
}

#[test]
fn crabbox_config_fails_when_the_file_is_readable_by_others() {
    let dir = tempfile::tempdir().unwrap();
    let rig = config_in(dir.path(), 0o644);
    let c = rig.with(host::crabbox_config);
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(c.detail.contains("644"), "{c:?}");
    assert!(c.fix.contains("chmod 600"), "{c:?}");
}

#[test]
fn crabbox_config_fails_when_a_parent_directory_is_group_writable() {
    let dir = tempfile::tempdir().unwrap();
    let rig = config_in(dir.path(), 0o600);
    mode(&dir.path().join("crabbox"), 0o775);
    let c = rig.with(host::crabbox_config);
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(c.fix.contains("chmod go-w"), "{c:?}");
}

#[test]
fn crabbox_config_may_be_absent() {
    let dir = tempfile::tempdir().unwrap();
    let rig = rig_with(&[("XDG_CONFIG_HOME", dir.path().to_str().unwrap())]);
    let c = rig.with(host::crabbox_config);
    assert_eq!(c.status, Status::Ok, "{c:?}");
}

#[test]
fn crabbox_config_follows_crabbox_config_over_the_xdg_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("elsewhere.yaml");
    fs::write(&file, "x: 1\n").unwrap();
    mode(&file, 0o666);
    let rig = rig_with(&[("CRABBOX_CONFIG", file.to_str().unwrap())]);
    let c = rig.with(host::crabbox_config);
    assert_eq!(c.status, Status::Fail, "{c:?}");
}
