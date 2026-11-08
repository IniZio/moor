//! Tests for the activity decider.

use super::{
    decide, may_apply_quiet, AgentState, BlockKind, Life, Note, Probe, Question, Source, Status,
    Term,
};
use crate::seam::clock::Ts;

const NOW: i64 = 1_000_000_000;
const OLD: Ts = Ts::from_millis(NOW - 3_600_000);
const YOUNG: Ts = Ts::from_millis(NOW - 1000);

fn term(status: Option<Status>) -> Term {
    Term {
        life: Life::Live,
        status,
        status_at: Some(Ts::from_millis(NOW - 1000)),
        turn: 1,
    }
}

fn read(status: Option<Status>, note: Note, screen: &str) -> Probe {
    Probe::Read {
        term: Some(term(status)),
        note,
        screen: Some(screen.to_owned()),
    }
}

fn state(probe: &Probe, started: Ts) -> AgentState {
    decide(probe, Ts::from_millis(NOW), started, None).state
}

fn blocked(kind: Option<BlockKind>, msg: &str) -> Status {
    Status::Blocked {
        kind,
        msg: Some(msg.to_owned()),
    }
}

#[test]
fn the_agents_own_status_gives_the_state() {
    let working = Some(Status::Working {
        doing: Some("Bash ls".to_owned()),
    });
    assert_eq!(
        state(&read(working, Note::default(), ""), OLD),
        AgentState::Working
    );
    assert_eq!(
        state(&read(Some(Status::Idle), Note::default(), ""), OLD),
        AgentState::Idle
    );
    let failed = Some(Status::Error { msg: None });
    assert_eq!(
        state(&read(failed, Note::default(), ""), OLD),
        AgentState::Idle
    );
}

#[test]
fn a_permission_asks_with_its_message_and_the_menu_on_screen() {
    let screen = "Run ls?\n❯ 1. Yes\n  2. No\n";
    let probe = read(
        Some(blocked(Some(BlockKind::Permission), "approve Bash: ls")),
        Note::default(),
        screen,
    );
    assert_eq!(
        state(&probe, OLD),
        AgentState::NeedsInput {
            question: Some("approve Bash: ls".to_owned()),
            options: vec!["Yes".to_owned(), "No".to_owned()],
        }
    );
}

#[test]
fn a_question_takes_its_text_and_choices_from_the_note() {
    let note = Note {
        question: Some(Question {
            text: "Which fruit?".to_owned(),
            options: vec!["Apple".to_owned(), "Banana".to_owned()],
        }),
        ..Note::default()
    };
    let probe = read(
        Some(blocked(
            Some(BlockKind::Question),
            "answer: Which fruit? (Apple · Banana)",
        )),
        note,
        "",
    );
    assert_eq!(
        state(&probe, OLD),
        AgentState::NeedsInput {
            question: Some("Which fruit?".to_owned()),
            options: vec!["Apple".to_owned(), "Banana".to_owned()],
        }
    );
}

#[test]
fn a_permission_never_shows_a_stale_question() {
    let note = Note {
        question: Some(Question {
            text: "Old?".to_owned(),
            options: vec!["A".to_owned()],
        }),
        ..Note::default()
    };
    let probe = read(
        Some(blocked(Some(BlockKind::Permission), "approve Bash: ls")),
        note,
        "",
    );
    assert_eq!(
        state(&probe, OLD),
        AgentState::NeedsInput {
            question: Some("approve Bash: ls".to_owned()),
            options: Vec::new(),
        }
    );
}

#[test]
fn an_idle_agent_with_background_work_waits_on_it() {
    let note = Note {
        background: vec!["gh run watch".to_owned()],
        ..Note::default()
    };
    assert_eq!(
        state(&read(Some(Status::Idle), note, ""), OLD),
        AgentState::Waiting {
            on: vec!["gh run watch".to_owned()]
        }
    );
}

#[test]
fn a_finished_or_missing_terminal_is_an_exit() {
    let mut t = term(Some(Status::Working { doing: None }));
    t.life = Life::Finished { exit: Some(3) };
    let finished = Probe::Read {
        term: Some(t),
        note: Note::default(),
        screen: None,
    };
    assert_eq!(state(&finished, OLD), AgentState::Exited { code: Some(3) });
    let gone = Probe::Read {
        term: None,
        note: Note::default(),
        screen: None,
    };
    assert_eq!(state(&gone, OLD), AgentState::Exited { code: None });
}

#[test]
fn no_status_yet_is_starting_while_young_then_unknown() {
    let probe = read(None, Note::default(), "");
    assert_eq!(state(&probe, YOUNG), AgentState::Starting);
    assert_eq!(state(&probe, OLD), AgentState::Unknown);
}

#[test]
fn a_first_run_screen_before_any_status_needs_input() {
    let probe = read(
        None,
        Note::default(),
        "Do you trust the files in this folder?",
    );
    let got = decide(&probe, Ts::from_millis(NOW), YOUNG, Some("folder trust"));
    assert_eq!(got.source, Source::FirstRun);
    assert!(matches!(got.state, AgentState::NeedsInput { .. }));
}

#[test]
fn a_failed_read_is_unknown_and_never_quiets() {
    let got = decide(&Probe::TimedOut, Ts::from_millis(NOW), OLD, None);
    assert_eq!(got.state, AgentState::Unknown);
    assert!(!may_apply_quiet(&got, Some(&Probe::TimedOut)));
}

#[test]
fn only_the_agents_status_or_its_end_may_quiet_a_box() {
    let idle = read(Some(Status::Idle), Note::default(), "");
    let got = decide(&idle, Ts::from_millis(NOW), OLD, None);
    assert!(may_apply_quiet(&got, Some(&idle)));
    let none = read(None, Note::default(), "");
    let got = decide(&none, Ts::from_millis(NOW), OLD, None);
    assert!(!may_apply_quiet(&got, Some(&none)));
    let gone = Probe::Read {
        term: None,
        note: Note::default(),
        screen: None,
    };
    let got = decide(&gone, Ts::from_millis(NOW), OLD, None);
    assert!(may_apply_quiet(&got, Some(&gone)));
}
