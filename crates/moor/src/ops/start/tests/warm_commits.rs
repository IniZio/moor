//! A Box at another commit than the checkpoint it forks: every line runs again, nothing it did is
//! saved, a line that fails there gets one start from scratch, and the opt-out keeps a chain per
//! commit.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ProviderStep;
use moor_core::seam::driver::ExecOutcome;
use moor_core::{BoxId, BoxName, BoxReq, CloseReq, StartReq};

use super::warm::{after_a_partial_setup, events, lines_on, row, start, LINES};
use super::{agentless_req, repo, repo_req, world, World};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::{resume_setup, warm_disk_note};
use crate::ops::start::launch::launch_req;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

const EXIT_1: ExecOutcome = ExecOutcome::Exited { code: 1 };

fn forks(w: &World) -> usize {
    let calls = w.driver.calls();
    calls.iter().filter(|c| c.starts_with("fork")).count()
}

fn acquires(w: &World) -> usize {
    w.driver.calls().iter().filter(|c| *c == "acquire").count()
}

#[test]
fn a_box_at_another_commit_forks_the_deepest_layer_and_reruns_every_line_saving_nothing() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let saved = w.driver.checkpoint_names();
    let moved = start(&w, "moved", "def456", LINES);
    assert!(w.driver.calls().contains(&"fork ckpt-2".to_owned()));
    assert_eq!(lines_on(&w, moved), ["one-a", "two-b", "three-c"]);
    let warm = events(&w, moved, "warm_started");
    assert_eq!(warm.len(), 1, "{warm:?}");
    assert_eq!(warm[0]["layer"], 2);
    assert_eq!(warm[0]["rerun_from"], "abc123");
    assert_eq!(row(&w, moved).lifecycle, BoxLifecycle::Ready);
    assert_eq!(w.driver.checkpoint_names(), saved, "a rerun saves nothing");
}

/// The setup attaches and log copies made on Box `id`'s machine.
fn setup_calls_on(w: &World, id: BoxId) -> (usize, usize) {
    let handle = row(w, id).boxed.unwrap().handle.unwrap();
    let execs = w.driver.execs();
    let on = execs.iter().filter(|(h, _, _)| *h == handle);
    let attaches = on.filter(|(_, s, _)| s.argv.join(" ").contains("moor setup: $"));
    let copies = w.driver.copies();
    let logs = copies
        .iter()
        .filter(|(h, from, _)| *h == handle && from.as_str().ends_with(".moor/setup.log"));
    (attaches.count(), logs.count())
}

#[test]
fn a_rerun_uses_one_attach_and_one_log_copy_however_many_lines_it_has() {
    for lines in [1_usize, 3, 7] {
        let list: Vec<String> = (0..lines).map(|i| format!("line-{i}")).collect();
        let yaml = format!("setup: [{}]\n", list.join(", "));
        let w = world(FakeDriver::new().with_checkpoints());
        let cold = start(&w, "cold", "abc123", &yaml);
        assert_eq!(setup_calls_on(&w, cold), (lines, lines), "a cold Box saves");
        let moved = start(&w, "moved", "def456", &yaml);
        assert_eq!(events(&w, moved, "warm_started").len(), 1);
        assert_eq!(setup_calls_on(&w, moved), (1, 1), "{lines} lines");
    }
}

#[test]
fn a_rerun_saves_no_checkpoint_even_for_a_key_nothing_holds_yet() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let saved = w.driver.checkpoint_names();
    let moved = start(&w, "moved", "def456", "setup: [one-z, two-b, three-c]\n");
    assert_eq!(events(&w, moved, "warm_started")[0]["layer"], 0);
    assert_eq!(lines_on(&w, moved), ["two-b", "three-c", "one-z"]);
    assert_eq!(w.driver.checkpoint_names(), saved);
}

#[test]
fn a_fork_whose_refresh_fell_back_to_a_fresh_clone_runs_every_line_and_saves_nothing() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().cache_volumes());
    let saved = w.driver.checkpoint_names();
    w.driver
        .push_exec_stdout("moor: work tree cold after: git fetch failed\nmoor.clone=base\n");
    let again = start(&w, "again", "abc123", LINES);
    assert_eq!(lines_on(&w, again), ["one-a", "two-b", "three-c"]);
    assert_eq!(w.driver.checkpoint_names(), saved);
    assert!(
        events(&w, again, "warm_rerun").is_empty(),
        "nothing is stale"
    );
}

#[test]
fn a_rerun_s_warm_start_keeps_the_steps_the_provider_reported() {
    let step = ProviderStep::parse("timing fork-total ms=24115").unwrap();
    let (w, _) = after_a_partial_setup(FakeDriver::new().with_fork_steps(vec![step]));
    let moved = start(&w, "moved", "def456", LINES);
    let warm = &events(&w, moved, "warm_started")[0];
    assert_eq!(warm["rerun_from"], "abc123");
    assert_eq!(warm["steps"][0]["name"], "fork-total");
}

#[test]
fn a_retry_on_a_rerun_fork_stays_a_rerun_and_saves_nothing() {
    let dies = ExecOutcome::Exited { code: 72 };
    let driver = FakeDriver::new().exec_ends_once_when("moor_tree_refresh\n", dies);
    let (w, _) = after_a_partial_setup(driver);
    let saved = w.driver.checkpoint_names();
    w.forge.answer(repo(), Ok(repo_at("def456", Some(LINES))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("moved")
    };
    assert!(run(&w.cx, &req).is_err(), "the fork's clone exec dies");
    let retry = BoxReq {
        box_name: crate::testing::box_name("moved"),
    };
    let v = crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(w.driver.checkpoint_names(), saved);
    let named = v.warnings_inline.iter().any(|n| n.contains("abc123"));
    assert!(named, "{:?}", v.warnings_inline);
}

#[test]
fn a_restart_after_a_rerun_gave_up_but_before_its_machine_went_stays_a_rerun() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let saved = w.driver.checkpoint_names();
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    w.driver.on_exec_of("one-a", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    w.forge.answer(repo(), Ok(repo_at("def456", Some(LINES))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("moved")
    };
    assert!(catch_unwind(AssertUnwindSafe(|| drop(run(&w.cx, &req)))).is_err());
    dying.store(false, Ordering::SeqCst);
    let store = w.cx.store().unwrap();
    let name = crate::testing::box_name("moved");
    let moved = store.open_box_by_name(&name).unwrap().unwrap().id;
    let gave_up = ExtraEvent::WarmRerunFailed {
        line: "one-a".to_owned(),
        tail: String::new(),
    };
    store.apply(Decision::new(moved).event(gave_up)).unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [moved]);
    resume_setup(&w.cx, store, moved).unwrap();
    assert_eq!(w.driver.checkpoint_names(), saved);
}

#[test]
fn a_line_that_moved_head_saves_no_checkpoint_and_says_so() {
    let moved_off = ExecOutcome::Exited { code: 1 };
    let driver = FakeDriver::new().exec_ends_when("moor_head=$(git", moved_off);
    let (w, first) = after_a_partial_setup(driver);
    let taken = events(&w, first, "checkpoint_taken");
    assert_eq!(taken.len(), 1, "the clone alone: {taken:?}");
    let warned = w.cx.take_warnings();
    let skipped = warned.iter().find(|x| x.code == "checkpoint_skipped");
    assert!(
        skipped.unwrap().message.contains("moved HEAD"),
        "{warned:?}"
    );
}

#[test]
fn a_box_at_the_checkpoint_s_own_commit_records_no_rerun() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let again = start(&w, "again", "abc123", LINES);
    assert_eq!(lines_on(&w, again), ["three-c"]);
    let warm = events(&w, again, "warm_started");
    assert!(warm[0].get("rerun_from").is_none(), "{warm:?}");
    assert!(events(&w, again, "warm_rerun").is_empty());
}

#[test]
fn a_rerun_forces_a_refresh_of_the_tree_only_on_the_fork_that_needs_it() {
    let (w, first) = after_a_partial_setup(FakeDriver::new());
    let again = start(&w, "again", "abc123", LINES);
    let moved = start(&w, "moved", "def456", LINES);
    let refreshes = |id| {
        let handle = row(&w, id).boxed.unwrap().handle.unwrap();
        let execs = w.driver.execs();
        let on = execs.iter().filter(|(h, _, _)| *h == handle);
        on.filter(|(_, s, _)| s.argv.join(" ").contains("moor_tree_refresh\n"))
            .count()
    };
    assert_eq!(refreshes(first), 0, "a cold Box has no old tree");
    assert_eq!(refreshes(again), 0, "the same commit skips as it did");
    assert_eq!(refreshes(moved), 1);
}

#[test]
fn across_commits_false_puts_the_commit_back_in_the_key() {
    let yaml = format!("{LINES}checkpoint: {{across_commits: false}}\n");
    let (w, _) = super::warm::after_a_partial_setup_of(FakeDriver::new(), &yaml);
    let saved = w.driver.checkpoint_names().len();
    let moved = start(&w, "moved", "def456", &yaml);
    assert!(events(&w, moved, "warm_started").is_empty());
    assert_eq!(forks(&w), 0);
    assert_eq!(lines_on(&w, moved), ["one-a", "two-b", "three-c"]);
    assert!(w.driver.checkpoint_names().len() > saved, "its own chain");
    let same = start(&w, "same", "def456", &yaml);
    assert_eq!(events(&w, same, "warm_started").len(), 1);
    assert!(events(&w, same, "warm_started")[0]
        .get("rerun_from")
        .is_none());
}

#[test]
fn a_line_that_fails_on_a_rerun_warns_gives_up_the_fork_and_comes_up_cold() {
    let driver = FakeDriver::new().exec_ends_once_when("three-c", EXIT_1);
    let (w, _) = after_a_partial_setup(driver);
    w.cx.take_warnings();
    let before = acquires(&w);
    w.driver
        .set_copy_text("moor setup: $ one-a\nmoor setup: $ two-b\nmoor setup: $ three-c\n");
    let moved = start(&w, "moved", "def456", LINES);
    let failed = events(&w, moved, "warm_rerun_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["line"], "three-c");
    let warned = w.cx.take_warnings();
    let warning = warned.iter().find(|x| x.code == "warm_rerun_failed");
    assert!(warning.unwrap().message.contains("three-c"), "{warned:?}");
    assert_eq!(acquires(&w), before + 1, "one start from scratch");
    let after = row(&w, moved);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(after.setup_failed_at.is_none());
    let boxed = after.boxed.unwrap();
    assert_eq!(boxed.state, BoxState::Ready);
    let fork = "box-2";
    assert_ne!(boxed.handle.unwrap().as_str(), fork);
    assert!(w.driver.calls().contains(&format!("release {fork}")));
}

#[test]
fn a_line_that_also_fails_cold_ends_setup_failed_naming_the_line() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().exec_ends_when("three-c", EXIT_1));
    let before = acquires(&w);
    let moved = start(&w, "moved", "def456", LINES);
    assert_eq!(acquires(&w), before + 1, "once, not a loop");
    assert_eq!(events(&w, moved, "warm_rerun_failed").len(), 1);
    let after = row(&w, moved);
    assert_eq!(after.lifecycle, BoxLifecycle::SetupFailed);
    let failed = events(&w, moved, "setup_failed");
    assert!(failed[0].to_string().contains("three-c"), "{failed:?}");
}

#[test]
fn a_box_that_gave_up_its_fork_no_longer_holds_that_checkpoint_back() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().exec_ends_when("three-c", EXIT_1));
    let moved = start(&w, "moved", "def456", LINES);
    assert_eq!(row(&w, moved).lifecycle, BoxLifecycle::SetupFailed);
    let first = CloseReq {
        target: BoxName::parse_allow_bare("first").unwrap().into(),
        force: true,
    };
    crate::ops::close::run(&w.cx, &first).unwrap();
    let provider = row(&w, moved).boxed.unwrap().provider;
    let store = w.cx.store().unwrap();
    let free = store.evictable(&provider).unwrap();
    assert!(free.iter().any(|e| e.checkpoint == "ckpt-2"), "{free:?}");
}

#[test]
fn a_retry_never_forks_a_checkpoint_of_another_commit() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().exec_ends_when("three-c", EXIT_1));
    let moved = start(&w, "moved", "def456", LINES);
    let forks_before = forks(&w);
    let reruns_before = events(&w, moved, "warm_rerun").len();
    let retry = BoxReq {
        box_name: crate::testing::box_name("moved"),
    };
    crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert_eq!(forks(&w), forks_before);
    assert_eq!(
        events(&w, moved, "warm_rerun").len(),
        reruns_before,
        "its machine is a cold one now"
    );
    assert_eq!(lines_on(&w, moved), ["one-a", "two-b", "three-c"]);
}

#[test]
fn a_retry_that_reruns_over_a_new_fork_says_so_in_its_reply() {
    let driver = FakeDriver::new()
        .exec_ends_once_when("three-c", EXIT_1)
        .exec_ends_once_when("three-c", EXIT_1);
    let (w, _) = after_a_partial_setup(driver);
    let moved = start(&w, "moved", "def456", LINES);
    assert_eq!(row(&w, moved).lifecycle, BoxLifecycle::SetupFailed);
    let gone = Decision::new(moved).box_input(BoxInput::DriverMissing);
    w.cx.store().unwrap().apply(gone).unwrap();
    let retry = BoxReq {
        box_name: crate::testing::box_name("moved"),
    };
    let v = crate::ops::retry::run(&w.cx, &retry).unwrap();
    assert!(v.setup.ok, "{v:?}");
    assert!(
        v.warnings_inline.iter().any(|n| n.contains("abc123")),
        "{:?}",
        v.warnings_inline
    );
}

#[test]
fn a_restart_during_a_rerun_runs_every_line_and_saves_nothing() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let saved = w.driver.checkpoint_names();
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    w.driver.on_exec_of("one-a", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    w.forge.answer(repo(), Ok(repo_at("def456", Some(LINES))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("moved")
    };
    let crashed = catch_unwind(AssertUnwindSafe(|| drop(run(&w.cx, &req))));
    assert!(crashed.is_err());
    dying.store(false, Ordering::SeqCst);
    let store = w.cx.store().unwrap();
    let name = crate::testing::box_name("moved");
    let moved = store.open_box_by_name(&name).unwrap().unwrap().id;
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [moved]);
    resume_setup(&w.cx, store, moved).unwrap();
    assert_eq!(lines_on(&w, moved), ["one-a", "two-b", "three-c"]);
    assert_eq!(row(&w, moved).lifecycle, BoxLifecycle::Ready);
    assert_eq!(w.driver.checkpoint_names(), saved);
}

#[test]
fn the_agent_of_a_rerun_box_is_told_of_the_disk_and_a_later_cold_machine_is_not() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    w.driver
        .push_exec_stdout("moor.clone=base\nmoor.warm.changed=2\nmoor.warm.ignored=dist/\n");
    w.forge.answer(repo(), Ok(repo_at("def456", Some(LINES))));
    let v = run(&w.cx, &repo_req("moved")).unwrap();
    let id = BoxId::new(v.boxed.id);
    let rerun = events(&w, id, "warm_rerun");
    assert_eq!(rerun[0]["commit"], "abc123");
    assert_eq!(rerun[0]["changed"], 2);
    let note = v.warnings_inline.iter().find(|n| n.contains("abc123"));
    let note = note.expect("the start reply names the disk's commit");
    assert!(
        note.contains("2 tracked paths") && note.contains("dist/"),
        "{note}"
    );
    let req = launch_req(&w.cx, &row(&w, id), None).unwrap();
    assert_eq!(req.disk_note.as_deref(), Some(note.as_str()));
    let store = w.cx.store().unwrap();
    for input in [BoxInput::DriverMissing, BoxInput::Requested] {
        store.apply(Decision::new(id).box_input(input)).unwrap();
    }
    assert_eq!(warm_disk_note(store, id), None);
}

#[test]
fn a_cold_box_tells_its_agent_nothing_of_a_disk() {
    let w = world(FakeDriver::new().with_checkpoints());
    w.forge.answer(repo(), Ok(repo_at("abc123", Some(LINES))));
    let v = run(&w.cx, &repo_req("first")).unwrap();
    let id = BoxId::new(v.boxed.id);
    let req = launch_req(&w.cx, &row(&w, id), None).unwrap();
    assert_eq!(req.disk_note, None);
}
