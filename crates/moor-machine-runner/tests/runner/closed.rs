//! A call naming a slot for a Box the controller no longer lists as open: a request already on
//! its way when the Box closed, or a Proxy answering from its last word from the controller.

use serde_json::{json, Value};

use crate::rig::{Rig, HEADERS};

const REFUSAL: &str = "Box 7 is closed.";

/// A runner that only sweeps at startup, with hooks and a relay that leave a mark on the machine.
fn marked() -> Rig {
    let rig = Rig::with_args(&["--relay", "fake", "--sweep-secs", "3600"]);
    rig.hook("prepare", "echo x >> ~/prepared; echo '{}'");
    rig.hook("suspend", "touch ~/suspended");
    let fake = env!("CARGO_BIN_EXE_fake-mcp");
    rig.hook("relays/fake", &format!("exec '{fake}'"));
    rig
}

fn refused(reply: &Value, tool: &str) {
    assert_eq!(reply["result"]["isError"], true, "{tool}: {reply}");
    assert_eq!(reply["result"]["content"][0]["text"], REFUSAL, "{tool}");
}

#[test]
fn every_call_of_a_closed_box_is_refused_and_touches_nothing() {
    let rig = marked();
    rig.open.lock().unwrap().clear();
    std::fs::remove_dir_all(rig.remote.join("moor/7")).unwrap();
    for tool in [
        "exec",
        "sync",
        "pull",
        "machine_info",
        "machine_claim",
        "machine_release",
        "fake_tap",
    ] {
        let args = json!({"command": "touch ~/ran", "path": "file", "dest": "file"});
        refused(&rig.call_reply(tool, args, &HEADERS), tool);
    }
    for mark in ["ran", "prepared", "fake-starts", "moor/7", "moor-state/7"] {
        assert!(!rig.remote.join(mark).exists(), "{mark} on the machine");
    }
    assert!(rig.rsyncs.lock().unwrap().is_empty());
}

#[test]
fn a_closed_box_lists_only_the_runners_own_tools_and_starts_nothing() {
    let rig = marked();
    rig.open.lock().unwrap().clear();
    let tools = rig.tools(&HEADERS);
    assert!(tools.iter().all(|t| t["name"] != "fake_tap"), "{tools:?}");
    for mark in ["prepared", "fake-starts"] {
        assert!(!rig.remote.join(mark).exists(), "{mark} on the machine");
    }
}

#[test]
fn a_closed_box_waiting_for_its_release_runs_no_hook() {
    let rig = marked();
    rig.call_with("machine_info", json!({}), &HEADERS).unwrap();
    rig.open.lock().unwrap().clear();
    let reply = rig.call_reply("machine_release", json!({}), &HEADERS);
    refused(&reply, "machine_release");
    assert!(!rig.remote.join("suspended").exists());
}
