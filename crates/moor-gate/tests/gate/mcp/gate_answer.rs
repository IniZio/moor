//! The tool a Box's owner answers a gate with.

use serde_json::json;

use super::mcp;

#[test]
fn a_read_token_cannot_answer_a_gate() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read");
    let result = m.call(
        &token,
        "answer_gate",
        json!({"box": "web/fix", "answer": "yes"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("moor:write"), "{text}");
    assert!(m.requests().is_empty());
}

#[test]
fn answer_gate_sends_the_answer_note_and_question_for_the_named_box() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "answer_gate",
        json!({"box": "web/fix", "answer": "no", "note": "use blue", "ask": "ab12"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let result = m.call(
        &token,
        "answer_gate",
        json!({"box": "web/fix", "answer": "yes"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    let sent: Vec<_> = m.requests().iter().map(|r| r["request"].clone()).collect();
    assert_eq!(
        sent,
        [
            json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
                "cmd": "gate-answer", "box": "web/fix", "answer": "no", "note": "use blue",
                "ask": "ab12"}}),
            json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
                "cmd": "gate-answer", "box": "web/fix", "answer": "yes"}}),
        ]
    );
}
