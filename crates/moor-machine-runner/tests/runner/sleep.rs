use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::rig::{caller, Rig};

/// Counts its runs, then prints a device and a metro port that depend on the slot.
const PREPARE: &str = r#"echo "$MOOR_BOX $MOOR_SLOT" >> ~/prepared; echo '{"device":"dev-'"$MOOR_BOX"'","metro":"'$((8081 + MOOR_PORT_OFFSET))'"}'"#;

/// Notes the Box, its slot, its device and its state dir.
const SUSPEND: &str =
    r#"echo "$MOOR_BOX $MOOR_SLOT $MOOR_FACT_DEVICE $MOOR_STATE_DIR" >> ~/suspended"#;

/// Notes the Box; once `~/need` runs (default 1) have happened, the machine has plenty of space.
const RELEASE: &str = r#"echo "$MOOR_BOX" >> ~/released; n=$(wc -l < ~/released); [ "$n" -ge "$(cat ~/need 2>/dev/null || echo 1)" ] && echo 30000000 > ~/.free-kb; true"#;

/// Headers for Box 7 after it woke in slot 3.
const SLOT_3: [(&str, &str); 4] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine", "minion"),
    ("X-Moor-Machine-Ssh", "moor@minion"),
    ("X-Moor-Machine-Slot", "3"),
];

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn sleepy(extra: &[&str]) -> Rig {
    let mut args = vec![
        "--prepare",
        PREPARE,
        "--suspend",
        SUSPEND,
        "--release",
        RELEASE,
        "--sweep-secs",
        "1",
    ];
    args.extend_from_slice(extra);
    Rig::with_args(&args)
}

fn record(rig: &Rig, id: i64) -> Value {
    let bytes = std::fs::read(rig.state().join(format!("boxes/{id}.json"))).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Prepares Box 7 and puts it to sleep; waits until it is suspended.
fn suspended_box() -> Rig {
    let rig = sleepy(&[]);
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "true"})).unwrap();
    rig.asleep.lock().unwrap().push(7);
    eventually("the suspend hook ran", || {
        !lines(&rig.remote.join("suspended")).is_empty()
    });
    rig
}

#[test]
fn an_asleep_box_is_suspended_once_with_its_facts() {
    let rig = suspended_box();
    let home = rig.remote.display();
    assert_eq!(
        lines(&rig.remote.join("suspended")),
        [format!("7 0 dev-7 {home}/moor-state/7")]
    );
    eventually("the record is marked", || {
        record(&rig, 7)["suspended_at"].is_u64()
    });
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(lines(&rig.remote.join("suspended")).len(), 1);
    assert!(rig.copy().is_dir(), "its files stay");
}

#[test]
fn a_failed_suspend_is_retried_and_leaves_the_record_unmarked() {
    let rig = Rig::with_args(&[
        "--prepare",
        PREPARE,
        "--suspend",
        "echo run >> ~/suspended; exit 1",
        "--sweep-secs",
        "1",
    ]);
    rig.call("exec", json!({"command": "true"})).unwrap();
    rig.asleep.lock().unwrap().push(7);
    eventually("the hook ran twice", || {
        lines(&rig.remote.join("suspended")).len() >= 2
    });
    assert!(record(&rig, 7)["suspended_at"].is_null());
}

#[test]
fn suspending_ends_the_holds_of_the_box() {
    let rig = sleepy(&["--resource", "build=1"]);
    let _running = rig.send_call(
        "exec",
        json!({"command": "touch ~/started; sleep 20", "hold": ["build"]}),
    );
    eventually("the command runs under its hold", || {
        rig.remote.join("started").exists()
    });
    rig.open.lock().unwrap().push(8);
    rig.asleep.lock().unwrap().push(7);
    eventually("the suspend hook ran", || {
        !lines(&rig.remote.join("suspended")).is_empty()
    });
    let other = caller(&rig, "8", "minion");
    let out = rig
        .call_with(
            "exec",
            json!({"command": "echo ok", "hold": ["build"], "hold_wait_secs": 2}),
            &other,
        )
        .unwrap();
    assert_eq!(out["output"], "ok\n");
}

#[test]
fn a_call_after_wake_in_another_slot_prepares_again_with_the_new_ports() {
    let rig = suspended_box();
    rig.asleep.lock().unwrap().clear();
    let info = rig.call_with("machine_info", json!({}), &SLOT_3).unwrap();
    assert_eq!(info["slot"], 3, "{info}");
    assert_eq!(info["port_offset"], 30, "{info}");
    assert_eq!(
        info["facts"],
        json!({"device": "dev-7", "metro": "8111"}),
        "{info}"
    );
    assert_eq!(lines(&rig.remote.join("prepared")), ["7 0", "7 3"]);
    assert!(record(&rig, 7)["suspended_at"].is_null());
    rig.call_with("machine_info", json!({}), &SLOT_3).unwrap();
    assert_eq!(lines(&rig.remote.join("prepared")).len(), 2);
}

#[test]
fn a_call_after_wake_in_the_same_slot_prepares_again_too() {
    let rig = suspended_box();
    rig.asleep.lock().unwrap().clear();
    let out = rig.call("exec", json!({"command": "true"}));
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(lines(&rig.remote.join("prepared")), ["7 0", "7 0"]);
}

#[test]
fn a_sync_after_wake_prepares_again() {
    let rig = suspended_box();
    rig.asleep.lock().unwrap().clear();
    rig.call_with("sync", json!({}), &SLOT_3).unwrap();
    assert_eq!(lines(&rig.remote.join("prepared")), ["7 0", "7 3"]);
}

/// Prepares Box `id` as a caller on the fake machine.
fn prepared(rig: &Rig, id: &'static str) {
    rig.open.lock().unwrap().push(id.parse().unwrap());
    let headers = caller(rig, id, "minion");
    rig.call_with("exec", json!({"command": "true"}), &headers)
        .unwrap();
}

/// Puts Box `id` to sleep and waits for its suspend hook; the next one is at least a second
/// later in the record.
fn sleep_and_wait(rig: &Rig, id: i64, runs: usize) {
    rig.asleep.lock().unwrap().push(id);
    eventually("the suspend hook ran", || {
        lines(&rig.remote.join("suspended")).len() >= runs
    });
    eventually("the record is marked", || {
        record(rig, id)["suspended_at"].is_u64()
    });
    std::thread::sleep(Duration::from_millis(1200));
}

#[test]
fn low_disk_evicts_the_oldest_sleeping_boxes_until_there_is_room() {
    let rig = sleepy(&[]);
    for id in ["8", "7", "9"] {
        prepared(&rig, id);
    }
    rig.set_free_kb(1_048_576);
    std::fs::write(rig.remote.join("need"), "2").unwrap();
    for (n, id) in [8, 7, 9].into_iter().enumerate() {
        sleep_and_wait(&rig, id, n + 1);
    }
    prepared(&rig, "10");
    assert_eq!(lines(&rig.remote.join("released")), ["8", "7"]);
    for id in ["8", "7"] {
        assert!(!rig.remote.join(format!("moor/{id}")).exists(), "{id}");
        assert!(
            !rig.remote.join(format!("moor-state/{id}")).exists(),
            "{id}"
        );
        let kept = record(&rig, id.parse().unwrap());
        assert_eq!(kept["prepared"], false, "{kept}");
        assert_eq!(kept["facts"], json!({}), "{kept}");
        assert_eq!(kept["evicted"], true, "{kept}");
    }
    assert!(
        rig.remote.join("moor/9").is_dir(),
        "the newest sleeper stays"
    );
    assert_eq!(record(&rig, 9)["prepared"], true);
}

#[test]
fn an_evicted_box_prepares_again_and_says_its_files_must_be_synced() {
    let rig = suspended_box();
    rig.set_free_kb(1_048_576);
    prepared(&rig, "10");
    assert!(!rig.copy().exists(), "evicted");
    rig.asleep.lock().unwrap().clear();
    let info = rig.call("machine_info", json!({})).unwrap();
    assert_eq!(info["facts"]["device"], "dev-7", "{info}");
    let notice = info["notice"].as_str().unwrap();
    assert!(
        notice.contains("removed") && notice.contains("sync"),
        "{notice}"
    );
    assert_eq!(lines(&rig.remote.join("prepared")).len(), 3);
    rig.write("a.txt", "a");
    rig.call("sync", json!({})).unwrap();
    let info = rig.call("machine_info", json!({})).unwrap();
    assert!(info["notice"].is_null(), "{info}");
}

#[test]
fn with_no_sleeping_box_left_the_call_fails_naming_free_space_and_the_floor() {
    let rig = sleepy(&[]);
    rig.call("sync", json!({})).unwrap();
    rig.call("exec", json!({"command": "true"})).unwrap();
    rig.set_free_kb(1_048_576);
    rig.open.lock().unwrap().push(10);
    let other = caller(&rig, "10", "minion");
    let err = rig
        .call_with("exec", json!({"command": "true"}), &other)
        .unwrap_err();
    assert!(
        err.contains("1.0 GB") && err.contains("20 GB") && err.contains("no sleeping Box"),
        "{err}"
    );
    assert!(rig.copy().is_dir(), "an awake Box keeps its files");
    assert!(lines(&rig.remote.join("released")).is_empty());
    let sync = rig.call_with("sync", json!({}), &other).unwrap_err();
    assert!(sync.contains("1.0 GB"), "{sync}");
}

#[test]
fn a_box_that_woke_is_not_evicted_before_its_first_call() {
    let rig = suspended_box();
    rig.asleep.lock().unwrap().clear();
    rig.set_free_kb(1_048_576);
    rig.open.lock().unwrap().push(10);
    let other = caller(&rig, "10", "minion");
    let err = rig
        .call_with("exec", json!({"command": "true"}), &other)
        .unwrap_err();
    assert!(err.contains("no sleeping Box"), "{err}");
    assert!(rig.copy().is_dir());
    assert!(lines(&rig.remote.join("released")).is_empty());
}

#[test]
fn a_floor_of_zero_skips_the_check() {
    let rig = sleepy(&["--min-free-gb", "0"]);
    rig.set_free_kb(1);
    rig.call("sync", json!({})).unwrap();
    let out = rig.call("exec", json!({"command": "echo ok"})).unwrap();
    assert_eq!(out["output"], "ok\n");
}

#[test]
fn a_call_in_another_slot_prepares_again_even_if_no_sweep_saw_the_sleep() {
    let rig = sleepy(&[]);
    rig.call("exec", json!({"command": "true"})).unwrap();
    let info = rig.call_with("machine_info", json!({}), &SLOT_3).unwrap();
    assert_eq!(info["facts"]["metro"], "8111", "{info}");
    assert_eq!(lines(&rig.remote.join("prepared")), ["7 0", "7 3"]);
    assert_eq!(record(&rig, 7)["slot"], 3);
}
