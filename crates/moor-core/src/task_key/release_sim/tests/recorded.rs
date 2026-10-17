//! Tests for deleting devices the Task recorded before use.

use std::fs;

use super::{
    calls, delete_devices_script, failed_lines, listing, make_sim_dir, setup_logs, stateful_xcrun,
    Scratch, A, B,
};

const R: &str = "DDDDDDDD-1111-2222-3333-444444444444";

#[test]
fn recorded_devices_are_deleted_before_the_set_wide_loop_and_logs_come_last() {
    let s = Scratch::new("recorded-order");
    let script = delete_devices_script(&s.layout(), &[R]);
    let rec = script.find(&format!("rec='{R}'")).unwrap();
    let rec_loop = script.find("for u in $rec; do del_dev").unwrap();
    let set_loop = script.find("for u in $(udids); do del_dev").unwrap();
    let logs = script.find("rm -rf \"$logs/$u\"").unwrap();
    assert!(
        rec < rec_loop && rec_loop < set_loop && set_loop < logs,
        "{script}"
    );
    assert!(!script.contains("shutdown booted") && !script.contains("delete all"));
}

#[test]
fn a_value_that_is_not_a_udid_is_reported_and_never_interpolated() {
    let s = Scratch::new("recorded-bad");
    let evil = "x'; touch pwned; '";
    let script = delete_devices_script(
        &s.layout(),
        &[evil, "not-a-udid", A.to_lowercase().as_str()],
    );
    assert!(!script.contains("rec='x"), "{script}");
    assert!(script.contains("rec=''"), "{script}");
    make_sim_dir(&s.layout());
    stateful_xcrun(&s, &[]);
    let (code, out) = s.run(&script);
    assert_eq!(code, Some(1), "{out}");
    let failed = failed_lines(&out);
    assert_eq!(failed.len(), 3, "{out}");
    assert!(
        failed.iter().all(|l| l.ends_with(": not a simulator UDID")),
        "{out}"
    );
    assert!(!s.path("pwned").exists());
}

#[test]
fn recorded_device_is_deleted_and_its_logs_removed_alongside_the_set() {
    let s = Scratch::new("recorded-run");
    let l = s.layout();
    make_sim_dir(&l);
    stateful_xcrun(&s, &[(R, "Booted"), (A, "Shutdown")]);
    let home = setup_logs(&s, &[R, A, B]);
    let (code, out) = s.run_home(&delete_devices_script(&l, &[R]), Some(&home));
    assert_eq!(code, Some(0), "{out}");
    let c = calls(&s);
    let first_delete = c.lines().find(|l| l.contains(" delete ")).unwrap();
    assert!(first_delete.ends_with(R), "{c}");
    assert!(!s.path(&format!("devs/{R}")).exists());
    assert!(!s.path(&format!("devs/{A}")).exists());
    for u in [R, A] {
        assert!(!s
            .path(&format!("login/Library/Logs/CoreSimulator/{u}"))
            .exists());
    }
    assert!(s
        .path(&format!("login/Library/Logs/CoreSimulator/{B}"))
        .exists());
}

#[test]
fn recorded_logs_are_removed_even_when_the_set_dir_is_gone() {
    let s = Scratch::new("recorded-gone");
    let l = s.layout();
    stateful_xcrun(&s, &[]);
    let home = setup_logs(&s, &[R]);
    let (code, out) = s.run_home(&delete_devices_script(&l, &[R]), Some(&home));
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(calls(&s), "");
    assert!(!s
        .path(&format!("login/Library/Logs/CoreSimulator/{R}"))
        .exists());
}

#[test]
fn recorded_device_already_gone_from_the_set_is_not_a_failure() {
    let s = Scratch::new("recorded-absent");
    let l = s.layout();
    make_sim_dir(&l);
    fs::write(s.path("unused"), listing()).unwrap();
    stateful_xcrun(&s, &[(A, "Shutdown")]);
    let home = setup_logs(&s, &[R]);
    let (code, out) = s.run_home(&delete_devices_script(&l, &[R]), Some(&home));
    assert_eq!(code, Some(0), "{out}");
    assert!(failed_lines(&out).is_empty(), "{out}");
}
