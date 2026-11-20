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
fn egress_forward_fails_when_the_binary_needs_a_loader_and_passes_when_it_is_static() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("moor-egress-forward");
    let state = dir.path().join("state");
    let rig = rig_with(&[
        ("MOOR_STATE_DIR", state.to_str().unwrap()),
        ("MOOR_EGRESS_FORWARD_BIN", bin.to_str().unwrap()),
    ]);
    let elf64 = crate::proxy::forwarder::elf::tests::elf64;
    fs::write(&bin, elf64(3, b"/lib64/ld-linux-x86-64.so.2\0")).unwrap();
    mode(&bin, 0o755);
    let c = rig.with(host::egress);
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(
        c.detail
            .ends_with("is dynamically linked (it needs /lib64/ld-linux-x86-64.so.2), so it cannot run in a Box"),
        "{c:?}"
    );
    assert!(c.fix.contains("unknown-linux-musl"), "{c:?}");
    fs::write(&bin, elf64(2, b"")).unwrap();
    assert_eq!(rig.with(host::egress).status, Status::Ok);
}

#[test]
fn kvm_is_checked_only_beside_an_installed_node_and_fails_when_boxes_cannot_open_it() {
    let dir = tempfile::tempdir().unwrap();
    let kvm = dir.path().join("kvm");
    fs::write(&kvm, "").unwrap();
    mode(&kvm, 0o600);
    let config = dir.path().join("config");
    let rig = rig_with(&[
        ("XDG_CONFIG_HOME", config.to_str().unwrap()),
        ("MOOR_KVM_DEVICE", kvm.to_str().unwrap()),
    ]);
    assert_eq!(rig.with(host::kvm), None, "no node installed here");
    let units = config.join("systemd/user");
    fs::create_dir_all(&units).unwrap();
    fs::write(units.join("moor-agent.service"), "").unwrap();
    let c = rig.with(host::kvm).unwrap();
    assert_eq!(c.status, Status::Fail, "{c:?}");
    assert!(
        c.detail
            .starts_with(&format!("{} is mode 0600", kvm.display())),
        "{c:?}"
    );
    assert!(c.fix.contains("docs/rootless-docker.md#kvm"), "{c:?}");
    mode(&kvm, 0o666);
    assert_eq!(rig.with(host::kvm).unwrap().status, Status::Ok);
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
