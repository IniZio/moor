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
    let b = answered_blocks("t", "U1", "q", "2. b", "U1");
    assert!(b.as_array().unwrap().iter().all(|x| x["type"] != "actions"));
    assert_eq!(b[1]["elements"][0]["text"], "Answered by <@U1>: 2. b");
}

#[test]
fn an_answer_that_holds_markup_is_escaped() {
    let b = answered_blocks("t", "U1", "q", "1. <!channel>", "U1");
    assert_eq!(
        b[1]["elements"][0]["text"],
        "Answered by <@U1>: 1. &lt;!channel&gt;"
    );
}
