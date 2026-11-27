//! The reading tool on the Box-facing listener: its description names the shipped intents, and a
//! hand-in reaches the fake controller for the Box the Proxy named.

use serde_json::json;

use super::{listener, text};

#[test]
fn hand_in_reading_names_each_intent_of_the_shipped_table() {
    let l = listener(true);
    let reply = l.rpc(Some("7"), "tools/list", json!({}));
    let tools = reply["result"]["tools"].as_array().unwrap();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "hand_in_reading")
        .unwrap();
    let said = tool["description"].as_str().unwrap();
    let table = moor_core::reading::IntentTable::shipped().unwrap();
    for intent in &table.intents {
        assert!(
            said.contains(&format!("- {}: {}", intent.name, intent.words)),
            "{said}"
        );
    }
    let phrase = &tool["inputSchema"]["$defs"]["PhraseDoc"]["properties"];
    assert!(
        phrase.get("intents").is_some() && phrase.get("holds").is_some(),
        "{tool}"
    );
}

#[test]
fn a_hand_in_reaches_the_controller_for_the_box_the_proxy_named_and_a_refusal_reaches_the_agent() {
    let l = listener(true);
    let call = |exact: &str| {
        let doc = json!({"phrases": [{"quote": {"exact": exact}}], "effects": []});
        l.rpc(
            Some("41"),
            "tools/call",
            json!({"name": "hand_in_reading", "arguments": doc}),
        )["result"]
            .clone()
    };
    let accepted = call("Never merge");
    assert_eq!(text(&accepted), "accepted", "{accepted}");
    let seen = l.seen.recv().unwrap();
    assert_eq!(seen["cmd"], "reading-hand-in");
    assert_eq!(seen["box_id"], 41);
    assert_eq!(
        seen["reading"]["phrases"][0]["quote"]["exact"],
        "Never merge"
    );
    let refused = call("nowhere");
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(text(&refused).contains("`nowhere`"), "{refused}");
}
