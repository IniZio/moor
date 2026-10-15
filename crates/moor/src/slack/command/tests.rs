//! Tests for app mention decisions.

use super::*;

/// Allow-list with one channel and one user.
fn allow() -> (Vec<String>, Vec<String>) {
    (vec!["C1".to_owned()], vec!["U1".to_owned()])
}

/// Runs `decide` for the allowed channel and user unless overridden.
fn run(channel: &str, user: &str, text: &str, bound: Option<&TaskName>) -> Action {
    let (channels, users) = allow();
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    decide(&allow, channel, user, text, "1730000000.123456", bound)
}

/// The usage reply.
fn usage() -> Action {
    Action::Reply("Usage: `@moor start <repo> <goal>`".to_owned())
}

#[test]
fn allowed_start() {
    let got = run("C1", "U1", "<@UBOT> start acme/api fix the bug", None);
    assert_eq!(
        got,
        Action::Start {
            name: TaskName::parse("slack-1730000000-123456").unwrap(),
            repo: "acme/api".to_owned(),
            goal: "fix the bug".to_owned(),
        }
    );
}

#[test]
fn thread_root_names_the_task() {
    let (channels, users) = allow();
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    let got = decide(&allow, "C1", "U1", "start r g", "1999999999.000001", None);
    assert!(
        matches!(&got, Action::Start { name, .. } if name.as_str() == "slack-1999999999-000001"),
        "got {got:?}"
    );
}

#[test]
fn disallowed_user_is_refused() {
    let got = run("C1", "U9", "<@UBOT> start r g", None);
    assert!(
        matches!(&got, Action::Reply(t) if t.contains("<@U9>") && t.contains("nothing was started")),
        "got {got:?}"
    );
}

#[test]
fn disallowed_channel_is_ignored() {
    assert_eq!(run("C9", "U1", "<@UBOT> start r g", None), Action::Ignore);
    assert_eq!(run("C9", "U9", "<@UBOT> start r g", None), Action::Ignore);
}

#[test]
fn bound_thread_refuses_second_start() {
    let bound = TaskName::parse("slack-1-2").unwrap();
    let got = run("C1", "U1", "<@UBOT> start r g", Some(&bound));
    assert!(
        matches!(&got, Action::Reply(t) if t.contains("`slack-1-2`")),
        "got {got:?}"
    );
}

#[test]
fn missing_goal_gets_usage() {
    assert_eq!(run("C1", "U1", "<@UBOT> start repo", None), usage());
    assert_eq!(run("C1", "U1", "<@UBOT> start repo   ", None), usage());
    assert_eq!(run("C1", "U1", "<@UBOT> start", None), usage());
}

#[test]
fn unknown_verb_and_empty_get_usage() {
    assert_eq!(run("C1", "U1", "<@UBOT> stop r g", None), usage());
    assert_eq!(run("C1", "U1", "<@UBOT>", None), usage());
}

#[test]
fn named_mention_form_is_stripped() {
    let got = run("C1", "U1", "<@U1|moor> <@U2> START r do it", None);
    assert!(
        matches!(got, Action::Start { ref repo, ref goal, .. } if repo == "r" && goal == "do it")
    );
}

#[test]
fn multiline_goal_is_preserved() {
    let got = run(
        "C1",
        "U1",
        "<@UBOT> start r line one\nline <https://x.io|two>\n",
        None,
    );
    assert!(
        matches!(&got, Action::Start { goal, .. } if goal == "line one\nline <https://x.io|two>"),
        "got {got:?}"
    );
}

#[test]
fn task_name_replaces_dot() {
    assert_eq!(
        task_name("1730000000.123456").unwrap().as_str(),
        "slack-1730000000-123456"
    );
}

#[test]
fn replies_name_the_task() {
    let name = TaskName::parse("slack-1-2").unwrap();
    assert_eq!(started(&name), "Started Task `slack-1-2`.");
    assert!(failed(&name, "boom").contains("boom"));
}

#[test]
fn reply_from_owner_steers() {
    assert_eq!(
        reply_action("U1", "U1", " <@UBOT> use tokio "),
        ReplyAction::Steer("use tokio".to_owned())
    );
}

#[test]
fn reply_from_other_user_is_refused_with_owner_mention() {
    assert_eq!(
        reply_action("U1", "U2", "use tokio"),
        ReplyAction::Refuse("Only <@U1> can steer this Task.".to_owned())
    );
}

#[test]
fn reply_close_closes_ignoring_case() {
    assert_eq!(reply_action("U1", "U1", "Close"), ReplyAction::Close);
    assert_eq!(
        reply_action("U1", "U1", "<@UBOT> close "),
        ReplyAction::Close
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
