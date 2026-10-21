//! Tests for thread message decisions and the front's start lines.

use super::*;

#[test]
fn replies_name_the_box() {
    let name = BoxName::parse_allow_bare("slack-1-2").unwrap();
    assert_eq!(started(&name), "Started Box `slack-1-2`.");
    assert!(failed(&name, "boom").contains("boom"));
}

#[test]
fn reply_from_the_starter_is_queued_without_its_mentions() {
    assert_eq!(
        reply_action("U1", "U1", " <@UBOT> use tokio "),
        ReplyAction::Queue("use tokio".to_owned())
    );
}

#[test]
fn reply_from_other_user_is_refused_with_the_starters_mention() {
    assert_eq!(
        reply_action("U1", "U2", "use tokio"),
        ReplyAction::Refuse("Only <@U1> can direct this thread.".to_owned())
    );
}

#[test]
fn a_reply_reading_close_is_queued_for_the_coordinator_as_text() {
    assert_eq!(
        reply_action("U1", "U1", "<@UBOT> close "),
        ReplyAction::Queue("close".to_owned())
    );
}

#[test]
fn a_reply_keeps_its_lines_and_drops_every_leading_mention_form() {
    assert_eq!(
        reply_action(
            "U1",
            "U1",
            "<@U1|moor> <@U2> line one\nline <https://x.io|two>\n"
        ),
        ReplyAction::Queue("line one\nline <https://x.io|two>".to_owned())
    );
}

#[test]
fn reply_with_no_text_is_ignored() {
    assert_eq!(reply_action("U1", "U1", " <@UBOT> "), ReplyAction::Ignore);
}

#[test]
fn reply_close_from_other_user_is_refused() {
    assert!(matches!(
        reply_action("U1", "U2", "close"),
        ReplyAction::Refuse(_)
    ));
}
