//! Tests for thread message decisions and the front's fixed lines and notices.

use super::*;

#[test]
fn replies_name_the_box() {
    let name = BoxName::parse_allow_bare("slack-1-2").unwrap();
    assert_eq!(started(&name, None), "Started Box `slack-1-2`.");
    assert_eq!(
        started(&name, Some("moor/x")),
        "Started Box `slack-1-2` on branch `moor/x`."
    );
    assert!(failed(&name, "boom").contains("boom"));
}

#[test]
fn a_branch_or_error_from_the_controller_cannot_ping_and_a_backticked_branch_is_posted_plain() {
    let name = BoxName::parse_allow_bare("slack-1-2").unwrap();
    assert_eq!(
        started(&name, Some("moor/<!channel> & y")),
        "Started Box `slack-1-2` on branch `moor/&lt;!channel&gt; &amp; y`."
    );
    assert_eq!(
        started(&name, Some("moor/<!channel>`x` & y")),
        "Started Box `slack-1-2` on branch moor/&lt;!channel&gt;`x` &amp; y."
    );
    assert_eq!(
        failed(&name, "<!here> no `repo` & <@U1>"),
        "Could not start Box `slack-1-2`: &lt;!here&gt; no `repo` &amp; &lt;@U1&gt;"
    );
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

#[test]
fn notices_are_fixed_templates_marked_as_the_fronts() {
    let name = BoxName::parse_allow_bare("slack-1-2").unwrap();
    let notices: Vec<String> = [
        BoxEvent::Started,
        BoxEvent::NotStarted,
        BoxEvent::Finished,
        BoxEvent::Closed,
    ]
    .iter()
    .map(|e| e.notice(&name))
    .collect();
    assert_eq!(
        notices,
        [
            "[moor] Box `slack-1-2` started.",
            "[moor] Box `slack-1-2` could not start.",
            "[moor] Box `slack-1-2` finished its turn.",
            "[moor] Box `slack-1-2` closed.",
        ]
    );
}
