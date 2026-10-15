//! Pane and multiplexer readings.

use super::said;
use super::{asking, exited, fresh, obs, waiting, Case, SILENT, TASKS};
use crate::activity::menu_options;
use crate::activity::Source as Src;
use crate::activity::{AgentState as S, HookKind as K, HookRead, PaneRead, PaneStatus as P};

#[test]
fn absent_hook_uses_herdr_status() {
    let a = || HookRead::Absent;
    Case::new("working", a(), obs(P::Working, "")).expect(S::Working, Src::Herdr);
    Case::new("idle is reported", a(), obs(P::Idle, "")).expect(S::Idle, Src::Herdr);
    Case::new(
        "blocked shows the profile's prompt line, not the footer",
        a(),
        asking(P::Blocked, "Allow edit?\n\nesc to interrupt", "Allow edit?"),
    )
    .expect(waiting("Allow edit?"), Src::Herdr);
    Case::new(
        "blocked without a prompt line shows nothing",
        a(),
        obs(P::Blocked, "out\nesc to interrupt"),
    )
    .expect(SILENT, Src::Herdr);
    Case::new(
        "blank prompt line shows nothing",
        a(),
        asking(P::Blocked, "", "  "),
    )
    .expect(SILENT, Src::Herdr);
}

#[test]
fn absent_hook_reads_the_pane_text() {
    let a = || HookRead::Absent;
    Case::new("idle with tasks", a(), obs(P::Idle, TASKS)).expect(S::Working, Src::Pane);
    Case::new("unknown with tasks", a(), obs(P::Unknown, TASKS)).expect(S::Working, Src::Pane);
    Case::new("unknown without", a(), obs(P::Unknown, "hello")).expect(S::Unknown, Src::Pane);
    Case::new("no pane", a(), PaneRead::Absent).expect(S::Unknown, Src::Pane);
    Case::new("pane timed out", a(), PaneRead::TimedOut).expect(S::Unknown, Src::Timeout);
}

#[test]
fn ended_pane_wins() {
    Case::new("exit over busy hook", fresh(K::Busy), exited(3))
        .expect(S::Exited { code: 3 }, Src::Pane);
    Case::new("exit while young", HookRead::Absent, exited(0))
        .started(1)
        .expect(S::Exited { code: 0 }, Src::Pane);
    Case::new("gone over busy hook", fresh(K::Busy), PaneRead::Gone).expect(S::Idle, Src::Pane);
}

const PERMISSION: &str = "Do you want to proceed?\n\
    ❯ 1. Yes\n  2. Yes, and don't ask again for rm commands\n  \
    3. No, and tell Claude what to do differently (esc)\n";

#[test]
fn menu_options_read_a_permission_menu() {
    assert_eq!(
        menu_options(PERMISSION),
        [
            "Yes",
            "Yes, and don't ask again for rm commands",
            "No, and tell Claude what to do differently (esc)"
        ]
    );
}

#[test]
fn menu_options_read_a_question_menu_with_a_footer() {
    let tail = "Which db?\n\n❯ 1. Postgres\n  2. SQLite\n  3. Type something.\n\nEnter to select · Esc to cancel\n";
    assert_eq!(
        menu_options(tail),
        ["Postgres", "SQLite", "Type something."]
    );
}

#[test]
fn menu_options_read_an_elicitation_dialog() {
    let tail = "Server wants input\n\n  1. Accept\n  2. Decline\n  3. Cancel\n";
    assert_eq!(menu_options(tail), ["Accept", "Decline", "Cancel"]);
}

#[test]
fn menu_options_take_the_last_menu_only() {
    let tail = "1. old\n2. older\n\nran things\n\n❯ 1. Yes\n  2. No\n";
    assert_eq!(menu_options(tail), ["Yes", "No"]);
}

#[test]
fn menu_options_need_numbers_counting_from_one() {
    assert!(menu_options("1. Yes\n3. No\n").is_empty());
    assert!(menu_options("2. Yes\n3. No\n").is_empty());
    assert!(menu_options("1. Only one\n").is_empty());
    assert!(menu_options("").is_empty());
}

#[test]
fn menu_options_ignore_prose() {
    assert!(menu_options("I did step 1. Then step 2. Done.\n").is_empty());
    assert!(menu_options("First, 1. do this and 2. do that\n").is_empty());
}

#[test]
fn needs_input_carries_the_menu_from_the_pane_tail() {
    let want = |q: &str| S::NeedsInput {
        question: Some(q.to_owned()),
        options: vec!["Yes".into(), "No".into()],
    };
    let tail = "Allow edit?\n❯ 1. Yes\n  2. No\n";
    Case::new(
        "herdr",
        HookRead::Absent,
        asking(P::Blocked, tail, "Allow edit?"),
    )
    .expect(want("Allow edit?"), Src::Herdr);
    Case::new(
        "hook",
        HookRead::Fresh(said("Allow edit?")),
        obs(P::Blocked, tail),
    )
    .expect(want("Allow edit?"), Src::Hooks);
}

#[test]
fn a_turn_ending_in_a_question_is_idle_not_needs_input() {
    let tail = "Done with the change.\nShall I proceed?\n";
    Case::new(
        "idle hook",
        fresh(K::Idle),
        asking(P::Idle, tail, "Shall I proceed?"),
    )
    .expect(S::Idle, Src::Hooks);
    Case::new("herdr only", HookRead::Absent, obs(P::Idle, tail)).expect(S::Idle, Src::Herdr);
}
