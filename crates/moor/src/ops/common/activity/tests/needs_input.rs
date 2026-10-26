//! Tests for the `needs_input` event written once per waiting episode.

use std::sync::Arc;

use moor_core::activity::{
    Activity, AgentState, HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes,
    Source,
};
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::agent_profile::FirstRunPrompt;
use moor_core::BoxId;

use super::{cx, rec, row, working_box, NOW, SESSION};
use crate::cx::Cx;
use crate::ops::common::activity::{read, settle, PaneLiveness, Reading, PROBE_DEADLINE};
use crate::store::{NewRun, RunOutcome};
use crate::testing::engine::FakeEngine;

fn reading(state: AgentState) -> Reading {
    let activity = Activity {
        state,
        source: Source::Pane,
    };
    Reading::new(Some(activity), PaneLiveness::Live, true, false)
}

fn asking(q: &str) -> Reading {
    reading(AgentState::NeedsInput {
        question: Some(q.to_owned()),
        options: vec!["1. Yes".to_owned(), "2. No".to_owned()],
    })
}

fn pass(cx: &Cx, id: BoxId, r: &Reading) {
    settle(cx, cx.store().unwrap(), &row(cx, id), r).unwrap();
}

fn kinds(cx: &Cx, id: BoxId) -> Vec<String> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn asked(cx: &Cx, id: BoxId) -> usize {
    kinds(cx, id).iter().filter(|k| *k == "needs_input").count()
}

fn setup() -> (tempfile::TempDir, Cx, BoxId) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), &Arc::new(FakeEngine::new()));
    let id = working_box(&cx, true);
    (dir, cx, id)
}

#[test]
fn one_event_per_waiting_episode_across_passes() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &asking("Proceed?"));
    assert_eq!(asked(&cx, id), 1);
}

#[test]
fn the_event_survives_the_idle_step_and_carries_the_prompt() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    let events = cx.store().unwrap().events(id, 100).unwrap();
    let ev = events.iter().find(|e| e.kind == "needs_input").unwrap();
    assert_eq!(
        ev.payload,
        serde_json::json!({"question": "Proceed?", "options": ["1. Yes", "2. No"]})
    );
    assert!(kinds(&cx, id).contains(&"idled".to_owned()));
}

#[test]
fn answering_ends_the_episode() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &reading(AgentState::Working));
    assert!(kinds(&cx, id).contains(&"busy".to_owned()));
    pass(&cx, id, &asking("Proceed?"));
    assert_eq!(asked(&cx, id), 2);
}

#[test]
fn a_different_question_without_a_resume_is_a_new_event() {
    let (_dir, cx, id) = setup();
    pass(&cx, id, &asking("Proceed?"));
    pass(&cx, id, &asking("Delete it?"));
    assert_eq!(asked(&cx, id), 2);
}

/// A probe whose hook reports the agent waiting on a menu question.
fn hook_asking() -> Probes {
    hook_probe(HookRecord {
        message: Some("Reuse PR #32?".to_owned()),
        options: vec!["Reuse".to_owned(), "Open new".to_owned()],
        doing: None,
        ..rec(HookKind::Blocked)
    })
}

fn hook_probe(record: HookRecord) -> Probes {
    Probes {
        hook: HookRead::Fresh(record),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

fn tick(cx: &Cx, id: BoxId) {
    let store = cx.store().unwrap();
    let r = read(cx, store, &row(cx, id), NOW, PROBE_DEADLINE).unwrap();
    settle(cx, store, &row(cx, id), &r).unwrap();
}

#[test]
fn a_run_while_the_agent_waits_keeps_one_episode() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook_asking()));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    tick(&cx, id);
    for _ in 0..3 {
        let new = NewRun {
            box_id: id,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            pid: 4242,
        };
        let (run, _claim) = store.begin_run(new).unwrap();
        store
            .apply(Decision::new(id).session(SessionInput::RunStarted))
            .unwrap();
        tick(&cx, id);
        let out = RunOutcome {
            exit: Some(0),
            interrupted: false,
            error: None,
        };
        store.finish_run(run, out).unwrap();
        tick(&cx, id);
    }
    let all = kinds(&cx, id);
    assert_eq!(all.iter().filter(|k| *k == "busy").count(), 3, "{all:?}");
    assert_eq!(asked(&cx, id), 1, "{all:?}");
}

#[test]
fn the_same_question_asked_again_after_work_is_a_new_episode() {
    let dir = tempfile::tempdir().unwrap();
    let mut again = hook_asking();
    if let HookRead::Fresh(r) = &mut again.hook {
        r.at = NOW;
    }
    let engine = FakeEngine::new()
        .then_probes(SESSION, hook_asking())
        .then_probes(SESSION, hook_probe(rec(HookKind::Busy)))
        .probes(SESSION, again);
    let (cx, _) = cx(dir.path(), &Arc::new(engine));
    let id = working_box(&cx, true);
    for _ in 0..4 {
        tick(&cx, id);
    }
    assert_eq!(asked(&cx, id), 2, "{:?}", kinds(&cx, id));
}

/// Claude Code's folder-trust screen as a pane showed it.
const TRUST_SCREEN: &str =
    include_str!("../../../../adapters/box_agent/claude/fixtures/folder_trust.txt");

/// An agent that has reported nothing since its launch marker, with `tail` on its pane.
fn at_launch(tail: &str) -> Probes {
    Probes {
        hook: HookRead::Fresh(rec(HookKind::Starting { launch: None })),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: tail.to_owned(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

fn trust_engine(quiet: bool) -> FakeEngine {
    let engine = FakeEngine::new()
        .prompt("trust this folder", FirstRunPrompt::FolderTrust)
        .probes(SESSION, at_launch(TRUST_SCREEN));
    if quiet {
        engine.hooks_quiet_during_first_run()
    } else {
        engine
    }
}

#[test]
fn a_first_run_screen_lets_the_box_idle_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), &Arc::new(trust_engine(true)));
    let id = working_box(&cx, true);
    tick(&cx, id);
    tick(&cx, id);
    assert_eq!(asked(&cx, id), 0, "{:?}", kinds(&cx, id));
    assert!(kinds(&cx, id).contains(&"idled".to_owned()));
}

#[test]
fn a_first_run_prompt_counts_only_for_an_agent_whose_hooks_wait_it_out() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = cx(dir.path(), &Arc::new(trust_engine(false)));
    let id = working_box(&cx, true);
    tick(&cx, id);
    assert!(!kinds(&cx, id).contains(&"idled".to_owned()));
}

#[test]
fn first_run_words_after_the_agent_reported_are_not_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut probes = at_launch(TRUST_SCREEN);
    probes.hook = HookRead::Fresh(rec(HookKind::Busy));
    let engine = trust_engine(true).probes(SESSION, probes);
    let (cx, _) = cx(dir.path(), &Arc::new(engine));
    let id = working_box(&cx, true);
    tick(&cx, id);
    assert_eq!(asked(&cx, id), 0, "{:?}", kinds(&cx, id));
}

#[test]
fn a_reading_waiting_on_a_prompt_or_first_run_screen_is_blocked() {
    let waiting = |source| {
        let activity = Activity {
            state: AgentState::NeedsInput {
                question: None,
                options: Vec::new(),
            },
            source,
        };
        Reading::new(Some(activity), PaneLiveness::Live, true, false)
    };
    assert!(waiting(Source::Hooks).blocked());
    assert!(waiting(Source::FirstRun).blocked());
    let idle = Activity {
        state: AgentState::Idle,
        source: Source::Hooks,
    };
    assert!(!Reading::new(Some(idle), PaneLiveness::Live, true, false).blocked());
}
