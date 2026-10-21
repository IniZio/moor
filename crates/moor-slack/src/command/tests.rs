//! Tests for app mention decisions.

use super::*;

/// Allow-list with one channel and one user.
fn allow() -> (Vec<String>, Vec<String>) {
    (vec!["C1".to_owned()], vec!["U1".to_owned()])
}

/// Runs `decide` for the allowed channel and user unless overridden.
fn run(channel: &str, user: &str, text: &str, existing: Option<&SlackBox>) -> Action {
    let (channels, users) = allow();
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    decide(&allow, channel, user, text, "1730000000.123456", existing)
}

/// Box `slack-1-2` of thread `1.2` in `state`.
fn row(state: BoxState) -> SlackBox {
    SlackBox {
        channel: "C1".to_owned(),
        thread_ts: "1.2".to_owned(),
        box_name: BoxName::parse_allow_bare("slack-1-2").unwrap(),
        repo: "r".to_owned(),
        goal: "g".to_owned(),
        requested_by: "U1".to_owned(),
        state,
    }
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
            name: BoxName::parse_allow_bare("slack-1730000000-123456").unwrap(),
            repo: "acme/api".to_owned(),
            goal: "fix the bug".to_owned(),
        }
    );
}

#[test]
fn thread_root_names_the_box() {
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
    let bound = row(BoxState::Bound {
        box_id: moor_core::name::BoxId::new(7),
        branch: None,
    });
    let got = run("C1", "U1", "<@UBOT> start r g", Some(&bound));
    assert!(
        matches!(&got, Action::Reply(t) if t.contains("already runs Box `slack-1-2`")),
        "got {got:?}"
    );
}

#[test]
fn starting_thread_says_its_box_is_still_starting() {
    let got = run(
        "C1",
        "U1",
        "<@UBOT> start r g",
        Some(&row(BoxState::Starting)),
    );
    assert!(
        matches!(&got, Action::Reply(t) if t.contains("`slack-1-2` is still starting") && !t.contains("runs")),
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
fn box_name_replaces_dot() {
    assert_eq!(
        box_name("1730000000.123456").unwrap().as_str(),
        "slack-1730000000-123456"
    );
}

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

/// Box `name` of thread `1.2`, asked for by `by`, in `state`.
fn asked(name: &str, by: &str, state: BoxState) -> SlackBox {
    SlackBox {
        box_name: BoxName::parse_allow_bare(name).unwrap(),
        requested_by: by.to_owned(),
        ..row(state)
    }
}

/// A bound state for Box id `id`.
fn bound(id: i64) -> BoxState {
    BoxState::Bound {
        box_id: moor_core::name::BoxId::new(id),
        branch: None,
    }
}

#[test]
fn close_targets_the_one_open_box_its_requester_asked_for() {
    let gone = BoxState::Gone {
        box_id: moor_core::name::BoxId::new(1),
        branch: None,
    };
    let boxes = [asked("old", "U1", gone), asked("new", "U1", bound(2))];
    let target = close_target(&boxes, "U1").unwrap();
    assert_eq!(target.box_name.as_str(), "new");
}

#[test]
fn close_with_no_open_box_or_with_several_closes_nothing() {
    assert_eq!(
        close_target(&[], "U1").err().unwrap(),
        "This thread has no open Box."
    );
    let two = [asked("a", "U1", bound(1)), asked("b", "U1", bound(2))];
    assert_eq!(
        close_target(&two, "U1").err().unwrap(),
        "This thread has 2 open Boxes. Ask the coordinator which to close."
    );
}

#[test]
fn close_from_someone_other_than_the_requester_closes_nothing() {
    let boxes = [asked("a", "U2", bound(1))];
    assert_eq!(
        close_target(&boxes, "U1").err().unwrap(),
        "Only <@U2> can close Box `a`."
    );
}
