//! The `ask` tool against the fake controller.

use serde_json::json;

use super::mcp;

#[test]
fn ask_sends_the_question_as_the_person_and_names_the_place_that_took_it() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    let result = m.call(
        &token,
        "ask",
        json!({"repos": ["acme/web", "acme/lib"], "prompt": "Where is GET /items handled?"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["box"], "place-1");
    let next = result["structuredContent"]["next"].as_str().unwrap();
    assert!(next.contains("box_status"), "{result}");
    let sent = m.requests();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0]["request"],
        json!({"cmd": "as", "identity": "authgear:alice-sub", "request": {
            "cmd": "ask", "repos": ["acme/web", "acme/lib"],
            "prompt": "Where is GET /items handled?"}})
    );
    let bad = m.call(&token, "ask", json!({"repos": ["web"], "prompt": "q"}));
    assert_eq!(bad["isError"], true, "{bad}");
}
