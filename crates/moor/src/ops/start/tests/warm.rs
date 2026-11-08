//! Tests for a new Box that starts from the deepest setup checkpoint an earlier Box saved.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::clone::AT_BASE;
use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::user::Identity;
use moor_core::{BoxId, StartReq};
use serde_json::Value;

use super::relaunch::{gone, session};
use super::{agentless_req, repo, req, world, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::store::BoxRow;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{put_secret, requester};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::repo_at;
use crate::vault::Vault;

pub(super) const LINES: &str = "setup: [one-a, two-b, three-c]\n";

/// Starts `name` of the repo at `base` whose `.moor.yaml` is `yaml`, with no agent.
pub(super) fn start(w: &World, name: &str, base: &str, yaml: &str) -> BoxId {
    w.forge.answer(repo(), Ok(repo_at(base, Some(yaml))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req(name)
    };
    let _ = run(&w.cx, &req);
    let store = w.cx.store().unwrap();
    let name = crate::testing::box_name(name);
    store.open_box_by_name(&name).unwrap().unwrap().id
}

pub(super) fn row(w: &World, id: BoxId) -> BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

/// The setup lines that ran on Box `id`'s machine, by their text.
pub(super) fn lines_on(w: &World, id: BoxId) -> Vec<&'static str> {
    let handle = row(w, id).boxed.unwrap().handle.unwrap();
    let ran: Vec<String> = w
        .driver
        .execs()
        .into_iter()
        .filter(|(h, _, _)| *h == handle)
        .map(|(_, spec, _)| spec.argv.join(" "))
        .filter(|argv| argv.contains("moor setup: $"))
        .collect();
    ["one-a", "two-b", "three-c", "one-z"]
        .into_iter()
        .filter(|l| ran.iter().any(|a| a.contains(l)))
        .collect()
}

/// The payloads of Box `id`'s events of `kind`.
pub(super) fn events(w: &World, id: BoxId, kind: &str) -> Vec<Value> {
    let store = w.cx.store().unwrap();
    let rows = store.newest_events_of(id, kind, 10).unwrap();
    rows.into_iter().map(|e| e.payload).collect()
}

/// A world whose first Box passes `one-a` and `two-b`, both slow enough to checkpoint, then
/// fails `three-c`; the fake's checkpoints are on.
pub(super) fn after_a_partial_setup(driver: FakeDriver) -> (World, BoxId) {
    after_a_partial_setup_of(driver, LINES)
}

/// Like [`after_a_partial_setup`] for a repo whose `.moor.yaml` is `yaml`.
pub(super) fn after_a_partial_setup_of(driver: FakeDriver, yaml: &str) -> (World, BoxId) {
    after_a_partial_setup_with(driver, yaml, |_| {})
}

/// Like [`after_a_partial_setup_of`], with `prepare` run on the world before its first Box.
pub(super) fn after_a_partial_setup_with(
    driver: FakeDriver,
    yaml: &str,
    prepare: impl FnOnce(&World),
) -> (World, BoxId) {
    let driver = driver
        .with_checkpoints()
        .exec_ends_once_when("three-c", ExecOutcome::Exited { code: 1 });
    let mut w = world(driver);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    w.cx = Cx::fake_with_clock(
        w.dir.path(),
        w.driver.clone(),
        w.proc.clone(),
        UserConfig::default(),
        clock.clone(),
    )
    .with_forge(w.forge.clone());
    w.driver.on_exec_of("two-b", move || {
        clock.sleep(Duration::from_secs(25));
    });
    prepare(&w);
    let first = start(&w, "first", "abc123", yaml);
    (w, first)
}

#[test]
fn a_new_box_forks_the_deepest_matching_layer_and_runs_only_the_lines_after_it() {
    let (w, first) = after_a_partial_setup(FakeDriver::new());
    let names = w.driver.checkpoint_names();
    assert_eq!(names.len(), 2, "the clone and the slow line: {names:?}");
    let second = start(&w, "second", "abc123", LINES);
    assert!(
        w.driver.calls().contains(&"fork ckpt-2".to_owned()),
        "{:?}",
        w.driver.calls()
    );
    assert_eq!(lines_on(&w, second), ["three-c"]);
    let (a, b) = (
        row(&w, first).boxed.unwrap(),
        row(&w, second).boxed.unwrap(),
    );
    assert_ne!(a.handle, b.handle);
    assert_eq!(
        b.workdir, a.workdir,
        "a fork keeps the saved disk's workdir"
    );
    let warm = events(&w, second, "warm_started");
    assert_eq!(warm.len(), 1, "{warm:?}");
    assert_eq!(warm[0]["layer"], 2);
    assert_eq!(warm[0]["layers"], 3);
    assert!(warm[0]["saved_ms"].as_u64().unwrap() >= 25_000, "{warm:?}");
}

#[test]
fn a_cold_start_records_each_checkpoint_it_takes_in_order_with_its_layer() {
    let (w, first) = after_a_partial_setup(FakeDriver::new());
    let taken = events(&w, first, "checkpoint_taken");
    assert_eq!(taken.len(), 2, "{taken:?}");
    let layers: Vec<_> = taken.iter().map(|t| &t["layer"]).collect();
    assert_eq!(layers, [&Value::from(0), &Value::from(2)]);
    let keys: Vec<_> = taken.iter().map(|t| t["key"].as_str().unwrap()).collect();
    assert_ne!(keys[0], keys[1]);
    assert_eq!(taken[0]["checkpoint"], "ckpt-1");
    assert_eq!(taken[1]["checkpoint"], "ckpt-2");
    assert!(taken.iter().all(|t| t["error"].is_null()), "{taken:?}");
}

#[test]
fn a_failed_checkpoint_is_recorded_with_its_error_and_setup_goes_on() {
    let failure = DriverError::Failed {
        summary: "disk full".to_owned(),
    };
    let (w, first) = after_a_partial_setup(FakeDriver::new().checkpoint_fails_once(failure));
    let taken = events(&w, first, "checkpoint_taken");
    assert_eq!(taken.len(), 2, "{taken:?}");
    let failed = &taken[0];
    assert_eq!(failed["layer"], 0);
    assert!(failed["checkpoint"].is_null());
    assert!(failed["error"].as_str().unwrap().contains("disk full"));
    assert_eq!(taken[1]["layer"], 2);
    assert!(taken[1]["error"].is_null());
}

#[test]
fn a_new_box_whose_first_line_changed_does_not_reuse_the_layers_above() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let changed = start(&w, "changed", "abc123", "setup: [one-z, two-b, three-c]\n");
    let warm = events(&w, changed, "warm_started");
    assert_eq!(warm.len(), 1, "only the clone still matches");
    assert_eq!(warm[0]["layer"], 0);
    assert_eq!(lines_on(&w, changed), ["two-b", "three-c", "one-z"]);
}

#[test]
fn a_fork_cut_by_a_lost_link_is_taken_over_and_only_the_lines_above_it_run() {
    let cut = DriverError::Transport {
        stderr_tail: "the stream ended: unexpected end of file".to_owned(),
    };
    let driver = FakeDriver::new().cut_acquires_go_on().fork_fails_once(cut);
    let (w, _) = after_a_partial_setup(driver);
    let second = start(&w, "second", "abc123", LINES);
    assert!(events(&w, second, "warm_start_failed").is_empty());
    assert!(events(&w, second, "setup_failed").is_empty());
    assert_eq!(events(&w, second, "warm_started").len(), 1);
    assert!(w.driver.calls().iter().any(|c| c == "adopt"));
    assert!(!lines_on(&w, second).contains(&"one-a"));
}

#[test]
fn a_failed_fork_sets_the_box_up_from_scratch_and_says_why() {
    let failure = DriverError::Failed {
        summary: "snapshot missing".to_owned(),
    };
    let (w, _) = after_a_partial_setup(FakeDriver::new().fork_fails_once(failure));
    let second = start(&w, "second", "abc123", LINES);
    let failed = events(&w, second, "warm_start_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["layer"], 2);
    let error = failed[0]["error"].as_str().unwrap();
    assert!(error.contains("snapshot missing"), "{error}");
    assert!(events(&w, second, "warm_started").is_empty());
    assert_eq!(lines_on(&w, second), ["one-a", "two-b", "three-c"]);
    let warned = w.cx.take_warnings();
    assert!(
        warned
            .iter()
            .any(|x| x.message.contains("snapshot missing")),
        "{warned:?}"
    );
}

#[test]
fn a_fork_carries_the_cache_plan_the_first_box_was_acquired_with() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().cache_volumes());
    let second = start(&w, "second", "abc123", LINES);
    assert_eq!(lines_on(&w, second), ["three-c"]);
    let acquired = w.driver.acquired_plans();
    let forked = w.driver.forked_plans();
    assert_eq!(forked.len(), 1);
    assert!(
        !forked[0].volumes().is_empty(),
        "presets give the Box volumes"
    );
    assert_eq!(forked[0], acquired[0]);
}

#[test]
fn a_new_box_under_another_cache_plan_does_not_reuse_the_layers() {
    let (w, _) = after_a_partial_setup(FakeDriver::new().cache_volumes());
    let yaml = format!("{LINES}cache: {{volumes: [{{name: sdk, path: /opt/sdk}}]}}\n");
    let other = start(&w, "other", "abc123", &yaml);
    assert!(events(&w, other, "warm_started").is_empty());
    assert!(w.driver.forked_plans().is_empty());
    assert_eq!(lines_on(&w, other), ["one-a", "two-b", "three-c"]);
}

#[test]
fn without_checkpoints_a_second_box_runs_its_whole_setup() {
    let w = world(FakeDriver::new());
    start(&w, "first", "abc123", LINES);
    let second = start(&w, "second", "abc123", LINES);
    assert!(events(&w, second, "warm_started").is_empty());
    assert!(!w
        .driver
        .calls()
        .iter()
        .any(|c| c.starts_with("fork") || c.starts_with("checkpoint")));
    assert_eq!(
        w.driver.calls().iter().filter(|c| *c == "acquire").count(),
        2
    );
    let ran = row(&w, second).boxed.unwrap().handle.unwrap();
    let execs = w.driver.execs();
    let setup = execs
        .iter()
        .filter(|(h, s, _)| *h == ran && s.argv.join(" ").contains("moor setup: $"));
    let joined: Vec<String> = setup.map(|(_, s, _)| s.argv.join(" ")).collect();
    assert_eq!(joined.len(), 1, "one script: {joined:?}");
    assert!(["one-a", "two-b", "three-c"]
        .iter()
        .all(|l| joined[0].contains(l)));
}

fn bob() -> Identity {
    Identity::local(2000)
}

/// Enrolls [`bob`] with the secrets a start needs.
fn enroll_bob(w: &World) {
    let store = w.cx.store().unwrap();
    let (user, _) = store.enroll(&bob(), None).unwrap();
    let vault = Vault::load_or_create(w.cx.vault_key().unwrap()).unwrap();
    for name in ["claude", "github"] {
        put_secret(store, &vault, user, name, &format!("{name}-of-2000"));
    }
}

#[test]
fn a_new_box_never_starts_from_a_checkpoint_another_users_box_saved() {
    let (mut w, _) = after_a_partial_setup(FakeDriver::new());
    enroll_bob(&w);
    w.cx = w.cx.with_requester(bob());
    let bobs = start(&w, "bobs", "abc123", LINES);
    assert!(events(&w, bobs, "warm_started").is_empty());
    assert!(!w.driver.calls().iter().any(|c| c.starts_with("fork")));
    assert_eq!(lines_on(&w, bobs), ["one-a", "two-b", "three-c"]);
    w.cx = w.cx.with_requester(requester());
    let alices = start(&w, "alices", "abc123", LINES);
    assert_eq!(events(&w, alices, "warm_started").len(), 1);
}

/// Starts a Box whose first line differs, so it forks only the clone and checkpoints the slow
/// `two-b` above it, with its clone printing `clone_says`; returns the checkpoints it saved.
fn warm_box_saved(clone_says: &str) -> Vec<String> {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let before = w.driver.checkpoint_names();
    w.driver.push_exec_stdout(clone_says);
    let warm = start(&w, "warm", "abc123", "setup: [one-z, two-b, three-c]\n");
    assert_eq!(events(&w, warm, "warm_started").len(), 1);
    let after = w.driver.checkpoint_names();
    after.into_iter().filter(|n| !before.contains(n)).collect()
}

#[test]
fn a_warm_box_whose_clone_did_not_report_the_base_saves_no_checkpoint() {
    assert_eq!(warm_box_saved("\n"), Vec::<String>::new());
}

#[test]
fn a_warm_box_whose_clone_reported_the_base_saves_checkpoints() {
    assert!(!warm_box_saved(&format!("{AT_BASE}\n")).is_empty());
}

#[test]
fn a_working_box_that_loses_its_machine_relaunches_on_its_deepest_checkpoint_with_its_agent() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let mut w = world(FakeDriver::new().with_checkpoints());
    w.cx = w.cx.with_engine(engine.clone());
    w.forge.answer(repo(), Ok(repo_at("abc123", Some(LINES))));
    let start_req = StartReq {
        repo: Some(repo()),
        prompt: Some("ship it".into()),
        ..req("demo")
    };
    let id = BoxId::new(run(&w.cx, &start_req).unwrap().boxed.id);
    let first = row(&w, id).boxed.unwrap().handle;
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    gone(&engine);
    let v = run(&w.cx, &start_req).unwrap();
    assert!(v.setup.ok && v.agent.launched, "{v:?}");
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert_ne!(after.boxed.unwrap().handle, first);
    let calls = w.driver.calls();
    assert_eq!(
        calls.iter().filter(|c| *c == "acquire").count(),
        1,
        "{calls:?}"
    );
    let warm = events(&w, id, "warm_started");
    assert_eq!(warm.len(), 1, "{warm:?}");
    assert_eq!(
        warm[0]["layer"], 3,
        "the deepest checkpoint covers every line"
    );
    assert!(lines_on(&w, id).is_empty());
    assert_eq!(engine.launches().len(), 2);
}
