use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::rig::Rig;

const RELEASE: &str =
    r#"echo "$MOOR_BOX $MOOR_STATE_DIR $TMPDIR ${MOOR_FACT_DEVICE-none}" >> ~/released"#;

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn reaping(release: &str) -> Rig {
    let rig = Rig::with_args(&["--sweep-secs", "1"]);
    rig.hook("release", release);
    rig
}

fn make(dir: &Path) {
    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    std::fs::write(dir.join("file"), "x").unwrap();
}

#[test]
fn exec_sees_a_tmpdir_inside_the_state_dir_of_its_box() {
    let rig = reaping(RELEASE);
    rig.call("sync", json!({})).unwrap();
    let out = rig
        .call(
            "exec",
            json!({"command": "echo $MOOR_STATE_DIR; echo $TMPDIR; touch $TMPDIR/shot"}),
        )
        .unwrap();
    let home = rig.remote.display();
    let want = format!("{home}/moor-state/7\n{home}/moor-state/7/tmp\n");
    assert_eq!(out["output"], want.as_str(), "{out}");
    assert!(rig.remote.join("moor-state/7/tmp/shot").is_file());
}

#[test]
fn releasing_a_recorded_box_removes_its_state_dir_too() {
    let rig = reaping(RELEASE);
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "touch $TMPDIR/shot"}))
        .unwrap();
    assert!(rig.remote.join("moor-state/7/tmp/shot").is_file());
    rig.open.lock().unwrap().clear();
    eventually("the state dir is gone", || {
        !rig.remote.join("moor-state/7").exists() && !rig.copy().exists()
    });
}

#[test]
fn a_host_mirror_of_a_closed_box_with_no_record_is_removed() {
    let rig = reaping(RELEASE);
    rig.call("sync", json!({})).unwrap();
    make(&rig.state().join("mirrors/9"));
    make(&rig.state().join("mirrors/other"));
    eventually("the mirror is gone", || {
        !rig.state().join("mirrors/9").exists()
    });
    assert!(rig.state().join("mirrors/7").is_dir(), "open Box's mirror");
    assert!(rig.state().join("mirrors/other").is_dir(), "not a Box");
}
