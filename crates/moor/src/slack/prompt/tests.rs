#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built blocks"
)]
use super::*;

fn opts(o: &[&str]) -> Vec<String> {
    o.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn open_blocks_have_one_button_per_option() {
    let b = open_blocks("fix-bug", "U1", "Which?", &opts(&["a", "b"]));
    let section = b[0]["text"]["text"].as_str().unwrap();
    assert!(section.contains("<@U1>") && section.contains("Which?"));
    let buttons = b[1]["elements"].as_array().unwrap();
    assert_eq!(buttons.len(), 2);
    assert_eq!(buttons[0]["action_id"], "answer_1");
    assert_eq!(buttons[1]["value"], "2");
    assert_eq!(buttons[1]["text"]["text"], "b");
    assert_eq!(b[1]["block_id"], "moor_prompt");
}

#[test]
fn question_cannot_ping() {
    let t = text("t", "U1", "ping <!channel> & <@U2>");
    assert!(t.starts_with("<@U1>"));
    assert!(t.contains("&lt;!channel&gt; &amp; &lt;@U2&gt;"));
}

#[test]
fn long_label_is_truncated_to_limit() {
    let long = "x".repeat(200);
    let b = open_blocks("t", "U1", "q", &[long]);
    let label = b[1]["elements"][0]["text"]["text"].as_str().unwrap();
    assert_eq!(label.chars().count(), 75);
    assert!(label.ends_with("..."));
}

#[test]
fn no_options_means_no_actions_block() {
    let b = open_blocks("t", "U1", "q", &[]);
    assert_eq!(b.as_array().unwrap().len(), 1);
    assert!(b[0]["text"]["text"]
        .as_str()
        .unwrap()
        .contains("Reply in this thread"));
}

#[test]
fn answered_blocks_drop_buttons() {
    let o = opts(&["a", "b"]);
    let b = answered_blocks("t", "U1", "q", &answer_label(&o, 2), "U1");
    assert!(b.as_array().unwrap().iter().all(|x| x["type"] != "actions"));
    assert_eq!(b[1]["elements"][0]["text"], "Answered by <@U1>: 2. b");
}

#[test]
fn answer_label_handles_range() {
    let o = opts(&["a"]);
    assert_eq!(answer_label(&o, 1), "1. a");
    assert_eq!(answer_label(&o, 0), "0");
    assert_eq!(answer_label(&o, 5), "5");
}

#[test]
fn fold_opens_replaces_and_clears() {
    let mut p = Pending::default();
    p.fold(
        1,
        "t",
        "needs_input",
        &json!({"question": "q1", "options": ["a"]}),
    );
    assert_eq!(p.open["t"].event_id, 1);
    p.fold(
        2,
        "t",
        "needs_input",
        &json!({"question": null, "options": []}),
    );
    assert_eq!(p.open["t"].event_id, 2);
    assert_eq!(p.open["t"].question, "");
    p.fold(3, "t", "box_ready", &json!({}));
    assert!(p.open.contains_key("t"));
    p.fold(4, "t", "working", &json!({}));
    assert!(p.open.is_empty());
}
