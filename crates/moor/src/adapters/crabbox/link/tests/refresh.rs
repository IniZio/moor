//! A login that lacks a group the Box user is now in gives way to a new one.

use super::*;
use crate::adapters::crabbox::link::STALE_GROUPS;

#[test]
fn a_login_missing_a_group_is_replaced_for_the_next_call() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.ssh.lock().unwrap().stale_groups = true;
    lab.driver().refresh_login(&handle()).unwrap();
    lab.ssh.lock().unwrap().stale_groups = false;
    assert_eq!(lab.connects(), 1, "the check rode the open login");
    lab.probe_tick().unwrap();
    assert_eq!(lab.connects(), 2, "the next call logged in anew");
}

#[test]
fn a_login_with_every_group_is_kept() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.driver().refresh_login(&handle()).unwrap();
    lab.probe_tick().unwrap();
    assert_eq!(lab.connects(), 1);
}

#[test]
fn a_box_without_a_shared_login_has_nothing_to_refresh() {
    let lab = lab();
    std::fs::create_dir(&lab.sockets).unwrap();
    std::fs::set_permissions(&lab.sockets, Permissions::from_mode(0o755)).unwrap();
    lab.driver().refresh_login(&handle()).unwrap();
    let ssh = lab.proc.calls().into_iter().filter(|c| c.tool == Tool::Ssh);
    assert_eq!(ssh.count(), 0);
}

#[test]
fn a_group_check_that_cannot_run_is_an_error_and_keeps_the_login() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.ssh.lock().unwrap().drop_next = true;
    assert!(lab.driver().refresh_login(&handle()).is_err());
}

/// Exit code of the group check with `id` answering `session` for the open login and `user` for
/// the group database.
fn group_check(session: &str, user: &str) -> Option<i32> {
    let dir = tempfile::tempdir().unwrap();
    let id = format!(
        "#!/bin/sh\ncase \"$*\" in -un) echo box ;; -G) echo '{session}' ;; '-G box') echo '{user}' ;; *) exit 9 ;; esac\n"
    );
    moor_testkit::write_executable(&dir.path().join("id"), id).unwrap();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", &crate::adapters::crabbox::link::stale_groups_script()])
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .output()
        .unwrap();
    out.status.code()
}

#[test]
fn the_group_check_fails_only_for_a_group_the_login_lacks() {
    assert_eq!(group_check("1000", "1000"), Some(0));
    assert_eq!(group_check("1000 27 2000", "27 1000 2000"), Some(0));
    assert_eq!(
        group_check("1000 2000", "1000"),
        Some(0),
        "a group PAM added"
    );
    assert_eq!(group_check("1000", "1000 2000"), Some(STALE_GROUPS));
    assert_eq!(group_check("100", "1000"), Some(STALE_GROUPS));
}
