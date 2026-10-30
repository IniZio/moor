use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::rig::Rig;

/// Counts its runs in `~/prepared`, says something on stderr, then prints the Box's device.
const PREPARE: &str = r#"echo "$MOOR_BOX $MOOR_SLOT $MOOR_PORT_OFFSET" >> ~/prepared; echo noise >&2; echo '{"device":"dev-'"$MOOR_BOX"'"}'"#;

/// Notes each run and the device it was given in `~/released`.
const RELEASE: &str = r#"echo "$MOOR_BOX $MOOR_FACT_DEVICE" >> ~/released"#;

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Waits up to 15 s for `done`.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn hooked(release: &str) -> Rig {
    let rig = Rig::with_args(&["--sweep-secs", "1"]);
    rig.hook("prepare", PREPARE);
    rig.hook("release", release);
    rig
}

#[test]
fn the_first_exec_prepares_once_and_its_facts_reach_the_command() {
    let rig = hooked(RELEASE);
    rig.call("sync", json!({})).unwrap();
    let out = rig
        .call(
            "exec",
            json!({"command": "echo \"device=$MOOR_FACT_DEVICE\""}),
        )
        .unwrap();
    assert!(
        out["output"].as_str().unwrap().contains("device=dev-7"),
        "{out}"
    );
    rig.call("exec", json!({"command": "true"})).unwrap();
    assert_eq!(lines(&rig.remote.join("prepared")), ["7 0 0"]);
}

#[test]
fn machine_info_prepares_and_returns_the_facts() {
    let rig = hooked(RELEASE);
    let info = rig.call("machine_info", json!({})).unwrap();
    assert_eq!(info["facts"], json!({"device": "dev-7"}), "{info}");
    assert!(
        rig.remote.join("moor/7").is_dir(),
        "prepare ran in the Box's copy"
    );
    rig.call("machine_info", json!({})).unwrap();
    assert_eq!(lines(&rig.remote.join("prepared")).len(), 1);
}

#[test]
fn a_bad_last_line_fails_the_call_and_the_next_call_prepares_again() {
    let prepare = r#"echo run >> ~/prepared; echo detail; if [ -f ~/good ]; then echo '{"device":"d"}'; else echo '{"device":1}'; fi"#;
    let rig = Rig::start();
    rig.hook("prepare", prepare);
    rig.call("sync", json!({})).unwrap();
    let err = rig.call("exec", json!({"command": "true"})).unwrap_err();
    assert!(err.contains("detail"), "the output tail: {err}");
    std::fs::write(rig.remote.join("good"), "").unwrap();
    let out = rig
        .call("exec", json!({"command": "echo $MOOR_FACT_DEVICE"}))
        .unwrap();
    assert_eq!(out["output"], "d\n", "{out}");
    assert_eq!(lines(&rig.remote.join("prepared")).len(), 2);
}

#[test]
fn a_failing_prepare_fails_the_call_with_its_output() {
    let rig = Rig::start();
    rig.hook("prepare", "echo broke; exit 4");
    let err = rig.call("machine_info", json!({})).unwrap_err();
    assert!(err.contains("broke") && err.contains('4'), "{err}");
}

#[test]
fn a_restarted_runner_does_not_prepare_again() {
    let mut rig = hooked(RELEASE);
    rig.call("machine_info", json!({})).unwrap();
    rig.restart();
    let info = rig.call("machine_info", json!({})).unwrap();
    assert_eq!(info["facts"], json!({"device": "dev-7"}));
    assert_eq!(lines(&rig.remote.join("prepared")).len(), 1);
}

#[test]
fn a_closed_box_is_released_once_and_its_record_and_mirror_go() {
    let rig = hooked(RELEASE);
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "true"})).unwrap();
    let (record, mirror) = (
        rig.state().join("boxes/7.json"),
        rig.state().join("mirrors/7"),
    );
    assert!(record.is_file() && mirror.is_dir());
    rig.open.lock().unwrap().clear();
    eventually("the record is gone", || !record.exists());
    assert!(!mirror.exists(), "the mirror is gone");
    assert!(!rig.copy().exists(), "the copy on the machine is gone");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(lines(&rig.remote.join("released")), ["7 dev-7"]);
}

#[test]
fn a_closed_box_that_only_synced_loses_its_copies_without_a_release() {
    let rig = hooked(RELEASE);
    rig.call("sync", json!({})).unwrap();
    let mirror = rig.state().join("mirrors/7");
    assert!(mirror.is_dir() && rig.copy().is_dir());
    rig.open.lock().unwrap().clear();
    eventually("the mirror is gone", || !mirror.exists());
    eventually("the copy on the machine is gone", || !rig.copy().exists());
    assert!(!rig.state().join("boxes/7.json").exists());
    assert!(
        lines(&rig.remote.join("released")).is_empty(),
        "never prepared, never released"
    );
}

#[test]
fn a_failing_release_keeps_the_record_and_mirror_and_retries() {
    let rig = hooked("echo run >> ~/released; exit 1");
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "true"})).unwrap();
    rig.open.lock().unwrap().clear();
    let released = rig.remote.join("released");
    eventually("release ran twice", || lines(&released).len() >= 2);
    assert!(rig.state().join("boxes/7.json").is_file());
    assert!(rig.state().join("mirrors/7").is_dir());
}

#[test]
fn an_open_box_is_never_released() {
    let rig = hooked(RELEASE);
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "true"})).unwrap();
    std::thread::sleep(Duration::from_secs(4));
    assert!(lines(&rig.remote.join("released")).is_empty());
    assert!(rig.state().join("boxes/7.json").is_file());
    assert!(rig.state().join("mirrors/7").is_dir());
}
