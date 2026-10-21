//! Tests for the context render: what always goes in, and what the budget drops.

use super::{render, Action, Parts, Said, Speaker};

fn person(text: &str) -> Said {
    Said {
        speaker: Speaker::Person("U1".to_owned()),
        text: text.to_owned(),
    }
}

fn reply(text: &str) -> Said {
    Said {
        speaker: Speaker::Coordinator,
        text: text.to_owned(),
    }
}

fn action() -> Action {
    Action {
        turn: 3,
        requested_by: "U1".to_owned(),
        tool: "start_box".to_owned(),
        args: format!("goal '{}'", "x".repeat(300)),
        outcome: Some("Box slack-1-1 started on branch moor/bump".to_owned()),
    }
}

#[test]
fn a_long_thread_keeps_its_first_request_and_journal_and_drops_old_messages() {
    let earlier: Vec<Said> = (0..200)
        .flat_map(|n| {
            [
                person(&format!("message {n:03}")),
                reply(&format!("answer {n:03}")),
            ]
        })
        .collect();
    let actions = [action()];
    let boxes = ["- Box `slack-1-1`, asked for by <@U1>: running".to_owned()];
    let new = [person("what is left?")];
    let parts = Parts {
        first_request: Some("bump the app version past 1.2.0"),
        actions: &actions,
        boxes: &boxes,
        earlier: &earlier,
        new: &new,
    };
    let text = render(&parts, 2048);
    assert!(text.len() <= 2048, "{} bytes", text.len());
    assert!(text.starts_with("## First request\nbump the app version past 1.2.0\n"));
    let journal = format!(
        "Turn 3, <@U1> asked. start_box goal '{} (shortened here; the Box got the full text). \
         Result: Box slack-1-1 started on branch moor/bump.",
        "x".repeat(194)
    );
    assert!(text.contains(&journal), "{text}");
    assert!(text.contains(&boxes[0]));
    assert!(text.ends_with("## New\n[<@U1>] what is left?\n"), "{text}");
    assert!(
        text.contains("[you] answer 199\n"),
        "the newest message is kept"
    );
    assert!(
        !text.contains("message 000"),
        "the oldest message is dropped"
    );
    let shown = text.matches("[<@U1>] message").count() + text.matches("[you] answer").count();
    let omitted = format!("{} earlier messages are not shown.\n", 400 - shown);
    assert!(text.contains(&omitted), "{text}");
    let newest_first = text.find("message 198").unwrap() < text.find("answer 199").unwrap();
    assert!(newest_first, "kept messages are in thread order");
}

#[test]
fn the_parts_that_always_go_in_stay_when_they_pass_the_budget() {
    let first = "y".repeat(10_000);
    let earlier = [person("old")];
    let new = [person(&"z".repeat(20_000))];
    let parts = Parts {
        first_request: Some(&first),
        actions: &[],
        boxes: &[],
        earlier: &earlier,
        new: &new,
    };
    let text = render(&parts, 1024);
    let first_shown = format!("{} [the rest is not shown]\n", "y".repeat(8 * 1024));
    assert!(text.contains(&first_shown));
    let new_shown = format!(
        "[<@U1>] {} [the rest is not shown]\n",
        "z".repeat(16 * 1024)
    );
    assert!(text.ends_with(&new_shown));
    assert!(text.contains("## Actions this thread took\nNone.\n"));
    assert!(text.contains("## Boxes now\nNone.\n"));
    assert!(text.contains("1 earlier messages are not shown.\n"));
    assert!(!text.contains("[<@U1>] old"));
}

#[test]
fn a_short_thread_shows_everything_with_no_count() {
    let earlier = [person("first ask"), reply("on it")];
    let new = [person("thanks")];
    let parts = Parts {
        first_request: None,
        actions: &[],
        boxes: &[],
        earlier: &earlier,
        new: &new,
    };
    let text = render(&parts, 32 * 1024);
    let want = "## First request\n(not recorded)\n\n## Actions this thread took\nNone.\n\n\
                ## Boxes now\nNone.\n\n## Earlier in the thread\n[<@U1>] first ask\n\
                [you] on it\n\n## New\n[<@U1>] thanks\n";
    assert_eq!(text, want);
}
