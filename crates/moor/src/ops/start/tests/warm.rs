//! Tests for a new Box that starts from the deepest setup checkpoint an earlier Box saved.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxId, StartReq};
use serde_json::Value;

use super::{agentless_req, repo, world, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::store::BoxRow;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

const LINES: &str = "setup: [one-a, two-b, three-c]\n";

/// Starts `name` of the repo at `base` whose `.moor.yaml` is `yaml`, with no agent.
fn start(w: &World, name: &str, base: &str, yaml: &str) -> BoxId {
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

fn row(w: &World, id: BoxId) -> BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

/// The setup lines that ran on Box `id`'s machine, by their text.
fn lines_on(w: &World, id: BoxId) -> Vec<&'static str> {
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
fn events(w: &World, id: BoxId, kind: &str) -> Vec<Value> {
    let store = w.cx.store().unwrap();
    let rows = store.newest_events_of(id, kind, 10).unwrap();
    rows.into_iter().map(|e| e.payload).collect()
}

/// A world whose first Box passes `one-a` and `two-b`, both slow enough to checkpoint, then
/// fails `three-c`; the fake's checkpoints are on.
fn after_a_partial_setup(driver: FakeDriver) -> (World, BoxId) {
    let driver = driver
        .with_checkpoints()
        .exec_ends_once_when("three-c", ExecOutcome::Exited { code: 1 });
    let mut w = world(driver);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    w.cx = Cx::fake_with_clock(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
        UserConfig::default(),
        clock.clone(),
    )
    .with_forge(w.forge.clone());
    w.driver.on_exec_of("two-b", move || {
        clock.sleep(Duration::from_secs(25));
    });
    let first = start(&w, "first", "abc123", LINES);
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
fn a_new_box_whose_base_or_first_line_changed_does_not_reuse_the_layers_above() {
    let (w, _) = after_a_partial_setup(FakeDriver::new());
    let moved = start(&w, "moved", "def456", LINES);
    assert!(events(&w, moved, "warm_started").is_empty());
    assert!(!w.driver.calls().iter().any(|c| c.starts_with("fork")));
    assert_eq!(lines_on(&w, moved), ["one-a", "two-b", "three-c"]);
    let changed = start(&w, "changed", "abc123", "setup: [one-z, two-b, three-c]\n");
    let warm = events(&w, changed, "warm_started");
    assert_eq!(warm.len(), 1, "only the clone still matches");
    assert_eq!(warm[0]["layer"], 0);
    assert_eq!(lines_on(&w, changed), ["two-b", "three-c", "one-z"]);
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
