//! `start_box` starts a Box with no agent: a task with words goes through `ask`.

use serde_json::json;

use super::mcp;

#[test]
fn start_box_takes_no_prompt_and_sends_nothing() {
    let m = mcp();
    let token = m.token("alice-sub", &m.url(), "moor:read moor:write");
    for extra in [json!({"prompt": "fix it"}), json!({"gates": []})] {
        let mut args = json!({"repos": ["acme/web"], "name": "web/fix"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let result = m.call(&token, "start_box", args);
        assert_eq!(result["isError"], true, "{extra}: {result}");
    }
    assert!(m.requests().is_empty());
}
