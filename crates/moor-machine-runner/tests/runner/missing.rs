use serde_json::{json, Value};

use crate::rig::Rig;

const REASON: &str =
    "machine mac has no free slot yet; this Box takes one as soon as another Box frees it";
const MISSING: [(&str, &str); 2] = [("X-Moor-Box", "7"), ("X-Moor-Machine-Missing", REASON)];

/// A runner with a prepare hook and a relay, both of which leave a mark on the machine.
fn marked() -> Rig {
    let rig = Rig::with_args(&["--relay", "fake"]);
    rig.hook("prepare", "echo x >> ~/prepared; echo '{}'");
    let fake = env!("CARGO_BIN_EXE_fake-mcp");
    rig.hook("relays/fake", &format!("exec '{fake}'"));
    rig
}

fn names(tools: &[Value]) -> Vec<String> {
    let mut names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_box_without_a_slot_lists_only_the_runners_own_tools() {
    let rig = marked();
    assert_eq!(
        names(&rig.tools(&MISSING)),
        [
            "exec",
            "machine_claim",
            "machine_info",
            "machine_release",
            "pull",
            "sync"
        ]
    );
}

#[test]
fn every_call_answers_the_reason_as_a_tool_error_and_touches_nothing() {
    let rig = marked();
    for tool in ["exec", "sync", "pull", "machine_info", "fake_anything"] {
        let reply = rig.call_reply(tool, json!({"command": "touch ~/ran"}), &MISSING);
        assert!(reply.get("error").is_none(), "{tool}: {reply}");
        assert_eq!(reply["result"]["isError"], true, "{tool}: {reply}");
        assert_eq!(
            reply["result"]["content"][0]["text"],
            format!("{REASON}. Try again later."),
            "{tool}"
        );
    }
    assert!(!rig.remote.join("ran").exists());
    assert!(std::fs::read_dir(rig.copy()).unwrap().next().is_none());
    assert!(rig.puts.lock().unwrap().is_empty());
    assert!(rig.mirror_excludes.lock().unwrap().is_empty());
}

#[test]
fn a_missing_marker_without_a_box_is_still_refused() {
    let rig = marked();
    let headers = [("X-Moor-Machine-Missing", REASON)];
    let err = rig.call_with("exec", json!({"command": "true"}), &headers);
    assert!(err.is_err());
}
