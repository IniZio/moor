use serde_json::{json, Value};

use super::prompt;
use crate::workflow::{draft_prompt, plan_first_prompt};

fn user(content: &Value) -> Value {
    with(content, &json!({}))
}

fn with(content: &Value, extra: &Value) -> Value {
    let mut v = json!({"type": "user", "uuid": "u1", "timestamp": "2026-11-19T10:00:00.000Z",
                       "message": {"role": "user", "content": content}});
    if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
        v.extend(extra.clone());
    }
    v
}

#[test]
fn words_typed_into_the_terminal_are_a_prompt_whether_a_string_or_text_blocks() {
    assert_eq!(
        prompt(&user(&json!("  fix the typo\n"))).as_deref(),
        Some("fix the typo")
    );
    let blocks = json!([{"type": "text", "text": "look at"}, {"type": "image"}, {"type": "text", "text": "this"}]);
    assert_eq!(prompt(&user(&blocks)).as_deref(), Some("look at\n\nthis"));
}

#[test]
fn what_claude_or_another_agent_added_is_not_a_prompt() {
    let result = json!([{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]);
    assert_eq!(prompt(&user(&result)), None);
    for text in [
        "<command-name>/model</command-name>",
        "<local-command-stdout>done</local-command-stdout>",
        "<agent-message from=\"a1\">hi</agent-message>",
        "[Request interrupted by user for tool use]",
        "   ",
    ] {
        assert_eq!(prompt(&user(&json!(text))), None, "{text}");
    }
    assert_eq!(
        prompt(&user(&json!("a <b> tag, and 1 < 2"))).as_deref(),
        Some("a <b> tag, and 1 < 2")
    );
    let meta = with(
        &json!("Stop hook feedback: keep going"),
        &json!({"isMeta": true}),
    );
    assert_eq!(prompt(&meta), None);
    let note = json!("a note from another session");
    let peer = with(&note, &json!({"origin": {"kind": "peer"}}));
    assert_eq!(prompt(&peer), None);
    let human = with(&json!("hello"), &json!({"origin": {"kind": "human"}}));
    assert_eq!(prompt(&human).as_deref(), Some("hello"));
}

#[test]
fn a_prompt_moor_built_is_not_the_persons() {
    assert_eq!(prompt(&user(&json!(draft_prompt("Bump the deps.")))), None);
    assert_eq!(
        prompt(&user(&json!(plan_first_prompt("Fix the typo.")))),
        None
    );
}
