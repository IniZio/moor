//! A front sees only its own root: the system dirs, its program, its `read_only` paths and its
//! state dir, the only place it may write. It cannot change metadata outside that dir, and no
//! Unix socket of the host exists for it.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde_json::Value;

use super::{shim, wait_for, Controller, PROBE};

#[test]
fn a_front_writes_its_own_state_dir_and_tmp() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["own"], "answers", "{got}");
    assert_eq!(got["own_dir"], "writable", "{got}");
    assert!(ctl.root.join("front").join("planted").exists());
    assert_eq!(got["tmp"], "writable", "{got}");
    assert!(ctl.root.join("front").join("tmp").join("planted").exists());
}

#[test]
fn a_front_writes_nothing_outside_its_state_dir() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    for absent in ["config", "moor_dir"] {
        assert_eq!(got[absent], "No such file or directory", "{absent}: {got}");
    }
    for shown in ["root", "home"] {
        assert_eq!(got[shown], "Read-only file system", "{shown}: {got}");
    }
    assert_eq!(got["home_secret"], "FileNotFoundError", "{got}");
}

#[test]
fn every_mount_the_front_reaches_is_read_only_but_its_state_dir() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    let own = ctl.root.join("front");
    let rw: Vec<&str> = got["rw_mounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|m| {
            !["/dev", "/proc"]
                .iter()
                .any(|c| Path::new(m).starts_with(c))
        })
        .collect();
    assert_eq!(rw, [own.to_str().unwrap(), "/tmp"], "{got}");
}

#[test]
fn a_front_changes_metadata_only_in_its_own_state_dir() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    for call in ["chmod", "chown", "utime", "setxattr", "fchmod"] {
        for file in ["shared_meta", "program_meta"] {
            assert_eq!(
                got[file][call], "Read-only file system",
                "{file} {call}: {got}"
            );
        }
        assert_ne!(
            got["own_meta"][call], "Read-only file system",
            "{call}: {got}"
        );
    }
    assert_eq!(got["own_meta"]["chmod"], "ok", "{got}");
    assert_eq!(got["fd3_chmod"], "Read-only file system", "{got}");
}

#[test]
fn a_front_reaches_no_unix_socket_of_the_host() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    for socket in ["tmp_sock", "run_sock", "bus", "docker", "lxd"] {
        assert_eq!(got[socket], "FileNotFoundError", "{socket}: {got}");
    }
    assert_eq!(got["shm"], "No such file or directory", "{got}");
    assert_eq!(got["own"], "answers", "{got}");
}

#[test]
fn the_state_dir_and_its_tmp_are_owner_only() {
    let ctl = Controller::start(PROBE, &[]);
    wait_for("the probe", || ctl.line("probe "));
    let tmp = ctl.root.join("front").join("tmp");
    let mode = fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn the_check_tries_a_write_the_sandbox_refuses() {
    let dir = crate::fresh_dir("check");
    let out = shim(&dir, &["--check"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "sandbox enforced: a write outside the allow-list was refused"
    );
}

#[test]
fn a_front_writes_dev_null_and_no_other_device() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["dev_null"], "writable", "{got}");
    assert_eq!(got["dev_zero"], "Permission denied", "{got}");
}

#[test]
fn a_front_reads_its_read_only_paths_and_runs_its_program_from_a_hidden_dir() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(got["shared_read"], 6, "{got}");
    assert_eq!(got["shared_write"], "Read-only file system", "{got}");
    assert_eq!(got["home_secret"], "FileNotFoundError", "{got}");
}

#[test]
fn a_front_resolves_names_and_trusts_the_system_certificates() {
    let ctl = Controller::start(PROBE, &[]);
    let got = wait_for("the probe", || ctl.line("probe "));
    let got: Value = serde_json::from_str(&got).unwrap();
    assert!(got["resolv"].as_u64().is_some_and(|n| n > 0), "{got}");
    assert_eq!(got["ca"], true, "{got}");
    assert_eq!(got["dns"], "ok", "{got}");
}
