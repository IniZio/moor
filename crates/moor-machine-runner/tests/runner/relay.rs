use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::rig::{caller, Rig, HEADERS};
use crate::{alive, gone_within_6s};

/// Prints device `dev-<box>` and metro port `800<box>`, except for Box 8, which gets no facts,
/// and Box 9, whose metro port is not a number.
const PREPARE: &str = r#"case "$MOOR_BOX" in 8) echo '{}';; 9) echo '{"device":"dev-9","metro_port":"none"}';; *) echo '{"device":"dev-'"$MOOR_BOX"'","metro_port":"800'"$MOOR_BOX"'"}';; esac"#;

/// Notes whether the fake's latest process still ran when release did.
const RELEASE: &str = r#"pid=$(tail -n 1 ~/fake-starts | cut -d ' ' -f 1); if kill -0 "$pid" 2>/dev/null; then echo alive; else echo gone; fi >> ~/released"#;

fn fake() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .parent()
        .unwrap()
        .join("examples/fake-mcp")
}

fn relayed(extra: &[&str]) -> Rig {
    let relay = format!("fake={}", fake().display());
    let mut args = vec![
        "--prepare",
        PREPARE,
        "--release",
        RELEASE,
        "--relay",
        &relay,
        "--force-arg",
        "fake.udid=device",
        "--force-arg",
        "fake.device_id=device",
        "--force-arg",
        "fake.devices=device",
        "--force-arg",
        "fake.port=metro_port",
        "--hide-tool",
        "fake.update",
        "--sweep-secs",
        "1",
    ];
    args.extend_from_slice(extra);
    Rig::with_args(&args)
}

fn lines(rig: &Rig, file: &str) -> Vec<String> {
    std::fs::read_to_string(rig.remote.join(file))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The pid of the fake's latest start.
fn latest_pid(rig: &Rig) -> u32 {
    let starts = lines(rig, "fake-starts");
    let last = starts.last().expect("the fake started");
    last.split(' ').next().unwrap().parse().unwrap()
}

/// Waits up to 15 s for `done`.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn tap(rig: &Rig, headers: &[(&str, &str)]) -> Result<Value, String> {
    rig.call_with("fake_tap", json!({"udid": "other", "x": 3}), headers)
}

#[test]
fn relayed_tools_are_listed_under_the_relay_name_without_the_forced_argument() {
    let rig = relayed(&[]);
    let tools = rig.tools(&HEADERS);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in [
        "sync",
        "exec",
        "machine_info",
        "fake_tap",
        "fake_list_devices",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    let tap = tools.iter().find(|t| t["name"] == "fake_tap").unwrap();
    let schema = &tap["inputSchema"];
    assert!(schema["properties"].get("udid").is_none(), "{schema}");
    assert!(schema["properties"].get("x").is_some(), "{schema}");
    assert!(
        !schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("udid")),
        "{schema}"
    );
    let devices = rig.call("fake_list_devices", json!({})).unwrap();
    assert_eq!(devices, json!({"devices": ["a", "b"]}));
}

#[test]
fn a_relayed_call_carries_the_box_fact_whatever_the_agent_sent() {
    let rig = relayed(&[]);
    let out = tap(&rig, &HEADERS).unwrap();
    assert_eq!(out, json!({"udid": "dev-7", "x": 3}));
    assert_eq!(lines(&rig, "fake-calls"), [r#"{"udid":"dev-7","x":3}"#]);
}

#[test]
fn a_box_without_the_fact_is_refused_naming_it_and_nothing_is_relayed() {
    let rig = relayed(&[]);
    let err = tap(&rig, &caller(&rig, "8", "minion")).unwrap_err();
    assert!(err.contains("device"), "{err}");
    assert!(lines(&rig, "fake-calls").is_empty(), "the fake saw no call");
}

#[test]
fn two_boxes_get_two_sessions_each_with_its_own_device() {
    let rig = relayed(&[]);
    rig.open.lock().unwrap().push(9);
    let other = caller(&rig, "9", "minion");
    assert_eq!(tap(&rig, &HEADERS).unwrap()["udid"], "dev-7");
    assert_eq!(tap(&rig, &other).unwrap()["udid"], "dev-9");
    let mut boxes: Vec<String> = lines(&rig, "fake-starts")
        .iter()
        .map(|l| l.split(' ').nth(1).unwrap().to_owned())
        .collect();
    boxes.sort();
    assert_eq!(boxes, ["7", "9"]);
}

#[test]
fn a_session_is_reused_across_calls() {
    let rig = relayed(&[]);
    rig.tools(&HEADERS);
    for _ in 0..3 {
        tap(&rig, &HEADERS).unwrap();
    }
    assert_eq!(lines(&rig, "fake-starts").len(), 1);
    assert_eq!(lines(&rig, "fake-calls").len(), 3);
}

#[test]
fn an_idle_session_ends_and_the_next_call_starts_it_again() {
    let rig = relayed(&["--relay-idle-secs", "2"]);
    tap(&rig, &HEADERS).unwrap();
    let pid = latest_pid(&rig);
    eventually("the idle fake ends", || !alive(pid));
    tap(&rig, &HEADERS).unwrap();
    assert_eq!(lines(&rig, "fake-starts").len(), 2);
}

#[test]
fn releasing_a_box_ends_its_session_before_release_runs() {
    let rig = relayed(&[]);
    tap(&rig, &HEADERS).unwrap();
    let pid = latest_pid(&rig);
    rig.open.lock().unwrap().clear();
    eventually("release runs", || !lines(&rig, "released").is_empty());
    assert!(!alive(pid), "the fake is gone");
    assert_eq!(lines(&rig, "released"), ["gone"]);
    tap(&rig, &HEADERS).unwrap();
    assert_eq!(lines(&rig, "fake-starts").len(), 2);
}

#[test]
fn a_crashed_server_starts_again_on_the_next_call() {
    let rig = relayed(&[]);
    tap(&rig, &HEADERS).unwrap();
    let pid = latest_pid(&rig);
    let killed = std::process::Command::new("kill")
        .args(["-KILL", "--", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    gone_within_6s(pid);
    let fifo_gone = || {
        std::fs::read_dir(&rig.remote).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ssh-in")
        })
    };
    eventually("the fake ssh exits", fifo_gone);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(tap(&rig, &HEADERS).unwrap()["udid"], "dev-7");
    assert_eq!(lines(&rig, "fake-starts").len(), 2);
}

#[test]
fn a_forced_integer_argument_arrives_as_a_json_number() {
    let rig = relayed(&[]);
    let out = rig
        .call_with("fake_connect", json!({"port": "1"}), &HEADERS)
        .unwrap();
    assert_eq!(out, json!({"port": 8007, "device_id": "dev-7"}));
    assert_eq!(
        lines(&rig, "fake-calls"),
        [r#"{"port":8007,"device_id":"dev-7"}"#]
    );
}

#[test]
fn a_fact_that_is_not_a_number_is_refused_naming_it_and_the_tool_and_nothing_is_relayed() {
    let rig = relayed(&[]);
    rig.open.lock().unwrap().push(9);
    let err = rig
        .call_with("fake_connect", json!({}), &caller(&rig, "9", "minion"))
        .unwrap_err();
    assert!(err.contains("metro_port"), "{err}");
    assert!(err.contains("fake_connect"), "{err}");
    assert!(lines(&rig, "fake-calls").is_empty(), "the fake saw no call");
}

#[test]
fn a_forced_array_argument_arrives_as_a_one_item_array() {
    let rig = relayed(&[]);
    let out = rig
        .call_with("fake_stop", json!({"devices": ["x", "y"]}), &HEADERS)
        .unwrap();
    assert_eq!(out, json!({"devices": ["dev-7"]}));
    assert_eq!(lines(&rig, "fake-calls"), [r#"{"devices":["dev-7"]}"#]);
}

#[test]
fn forced_arguments_of_every_type_are_hidden_from_the_schema() {
    let rig = relayed(&[]);
    let tools = rig.tools(&HEADERS);
    let connect = tools.iter().find(|t| t["name"] == "fake_connect").unwrap();
    assert_eq!(connect["inputSchema"]["properties"], json!({}));
    let stop = tools.iter().find(|t| t["name"] == "fake_stop").unwrap();
    assert_eq!(stop["inputSchema"]["properties"], json!({}));
}

#[test]
fn a_hidden_tool_is_not_listed_and_a_call_to_it_is_refused() {
    let rig = relayed(&[]);
    let tools = rig.tools(&HEADERS);
    assert!(tools.iter().any(|t| t["name"] == "fake_tap"));
    assert!(
        tools.iter().all(|t| t["name"] != "fake_update"),
        "{tools:?}"
    );
    let err = rig
        .call_with("fake_update", json!({}), &HEADERS)
        .unwrap_err();
    assert!(err.contains("hidden"), "{err}");
    assert!(lines(&rig, "fake-calls").is_empty(), "the fake saw no call");
}
