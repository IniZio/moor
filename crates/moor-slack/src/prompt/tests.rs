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

#[test]
fn a_question_that_escaping_grows_still_fits_a_section_and_splits_no_entity() {
    let question = "<".repeat(3000);
    let fallback = text("t", "U1", &question);
    let blocks = open_blocks("t", "U1", &question, &opts(&["a"]));
    let bare = open_blocks("t", "U1", &question, &[]);
    let answered = answered_blocks("t", "U1", &question, &format!("1. {question}"), "U1");
    let shown = [
        fallback.as_str(),
        blocks[0]["text"]["text"].as_str().unwrap(),
        bare[0]["text"]["text"].as_str().unwrap(),
        answered[0]["text"]["text"].as_str().unwrap(),
        answered[1]["elements"][0]["text"].as_str().unwrap(),
    ];
    for text in shown {
        assert!(
            text.chars().count() <= 3000,
            "{} chars",
            text.chars().count()
        );
        let head = text.split('\n').next().unwrap();
        let cut = head.strip_suffix("...").unwrap();
        assert!(cut.ends_with("&lt;"), "{:?}", cut.get(cut.len() - 8..));
    }
    assert!(bare[0]["text"]["text"]
        .as_str()
        .unwrap()
        .ends_with("\nReply in this thread to answer."));
}

#[test]
fn options_past_the_button_limit_are_listed_to_pick_by_number() {
    let options: Vec<String> = (1..=30).map(|n| format!("o<{n}>")).collect();
    let b = open_blocks("t", "U1", "q", &options);
    let buttons = b[1]["elements"].as_array().unwrap();
    assert_eq!(buttons.len(), 25);
    assert_eq!(buttons[24]["value"], "25");
    let more = b[2]["text"]["text"].as_str().unwrap();
    assert!(
        more.starts_with("Reply in this thread with the number"),
        "{more}"
    );
    let listed: Vec<&str> = more.lines().skip(1).collect();
    assert_eq!(listed.first(), Some(&"26. o&lt;26&gt;"));
    assert_eq!(listed.len(), 5);
    let few = open_blocks("t", "U1", "q", &options[..25]);
    assert_eq!(few.as_array().unwrap().len(), 2);
}
