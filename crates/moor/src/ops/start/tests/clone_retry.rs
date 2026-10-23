//! Tests for a start whose clone failed, and for the start that retries it in the kept Box.

use std::sync::{Arc, Mutex};

use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Intent;
use moor_core::{BoxId, ErrorKind, StartReq};

use super::setup::{exit, kinds, setup_execs, start_req};
use super::state::working;
use super::{hold, script_base_yaml, world, World};
use crate::adapters::clock::SystemClock;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{DriverError, ExecOutcome};

const YAML: &str = "setup: [a]\n";

/// A world over a repo with `setup: [a]`, whose first execs give `results`.
fn world_with_setup(
    results: impl IntoIterator<Item = Result<ExecOutcome, DriverError>>,
) -> (World, Arc<FakeEngine>) {
    let mut w = world(FakeDriver::new().exec_results(results));
    script_base_yaml(&w, Some(YAML));
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

fn start(w: &World) -> Result<moor_core::view::StartView, crate::error::CommandError> {
    let req = StartReq {
        goal: Some("ship it".into()),
        ..start_req()
    };
    run(&w.cx, &req)
}

fn field<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v.get(key).unwrap()
}

fn row(w: &World) -> crate::store::BoxRow {
    w.cx.store()
        .unwrap()
        .box_row(BoxId::new(1))
        .unwrap()
        .unwrap()
}

fn box_state(w: &World) -> Option<BoxState> {
    row(w).boxed.map(|b| b.state)
}

#[test]
fn a_failed_clone_keeps_the_box_and_says_how_to_debug_retry_and_close_it() {
    let (w, engine) = world_with_setup([Ok(exit(1))]);
    let err = start(&w).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err
        .error
        .to_string()
        .starts_with("cloning the repo into the Box failed: exit 1"));
    assert_eq!(
        err.hints,
        [
            "the clone step failed; Box box-1 is kept for debugging and the agent did not start",
            "`moor start demo` again retries on this Box",
            "`moor close demo` closes it; add --force to discard unpushed work in the Box",
        ]
    );
    assert!(engine.launches().is_empty());
    assert!(!w.driver.calls().iter().any(|c| c.starts_with("release")));
    assert_eq!(box_state(&w), Some(BoxState::Ready));
    assert_eq!(row(&w).lifecycle, BoxLifecycle::SetupFailed);
}

#[test]
fn a_failed_clone_names_the_step_the_tail_and_the_box_in_the_error_value() {
    let (w, _) = world_with_setup([Ok(exit(1))]);
    let err = start(&w).unwrap_err();
    let value = err.value.unwrap();
    assert_eq!(field(&value, "failed_step"), "clone");
    assert_eq!(field(&value, "handle"), "box-1");
    assert!(value.get("log_tail").is_none());
    let boxed = value.get("box").unwrap();
    assert_eq!(field(boxed, "lifecycle"), "setup_failed");
}

#[test]
fn a_failed_clone_shows_the_message_and_every_hint_in_text_and_the_fields_in_json() {
    let (w, _) = world_with_setup([Ok(exit(1))]);
    let err = start(&w).unwrap_err();
    let resp = err.into_response("start", Vec::new());
    let shown = crate::cli::render(&resp, &crate::cli::GlobalOpts::default());
    assert_eq!(shown.stdout, "");
    assert_eq!(
        shown.stderr,
        "moor: cloning the repo into the Box failed: exit 1\n\
         hint: the clone step failed; Box box-1 is kept for debugging and the agent did not start\n\
         hint: `moor start demo` again retries on this Box\n\
         hint: `moor close demo` closes it; add --force to discard unpushed work in the Box\n"
    );
    let opts = crate::cli::GlobalOpts {
        json: true,
        ..crate::cli::GlobalOpts::default()
    };
    let json: serde_json::Value =
        serde_json::from_str(&crate::cli::render(&resp, &opts).stdout).unwrap();
    assert_eq!(json.pointer("/value/failed_step"), Some(&"clone".into()));
    assert_eq!(json.pointer("/value/handle"), Some(&"box-1".into()));
    assert!(json.pointer("/value/log_tail").is_none());
}

#[test]
fn start_after_a_failed_clone_retries_in_the_same_box_and_launches() {
    let (w, engine) = world_with_setup([Ok(exit(1)), Ok(exit(0)), Ok(exit(0))]);
    start(&w).unwrap_err();
    let again = start(&w).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let acquires = w.driver.calls().iter().filter(|c| *c == "acquire").count();
    assert_eq!(acquires, 1);
    assert_eq!(setup_execs(&w).len(), 1);
    assert!(setup_execs(&w).iter().all(|e| e.0.as_str() == "box-1"));
    assert_eq!(engine.launches().len(), 1);
    let row = row(&w);
    assert!(working(&row));
    assert!(row.intent.is_none());
    assert!(kinds(&w, row.id).contains(&"setup_finished".to_owned()));
}

#[test]
fn a_retry_that_fails_to_clone_again_keeps_the_box_and_stays_setup_failed() {
    let (w, engine) = world_with_setup([Ok(exit(1)), Ok(exit(2))]);
    start(&w).unwrap_err();
    let err = start(&w).unwrap_err();
    assert!(err.error.to_string().contains("exit 2"), "{err:?}");
    assert_eq!(field(&err.value.unwrap(), "failed_step"), "clone");
    assert!(engine.launches().is_empty());
    let row = row(&w);
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
    assert!(row.intent.is_none() && row.setup_failed_at.is_some());
    assert_eq!(box_state(&w), Some(BoxState::Ready));
    let calls = w.driver.calls();
    assert_eq!(calls.iter().filter(|c| *c == "acquire").count(), 1);
    assert!(!calls.iter().any(|c| c.starts_with("release")));
}

#[test]
fn a_retry_in_the_kept_box_holds_the_setup_intent_while_it_runs() {
    let (w, _) = world_with_setup([Ok(exit(1))]);
    start(&w).unwrap_err();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join("moor.db"));
    w.driver.on_exec(move || {
        let store = Store::open(&path, Arc::new(SystemClock::real())).unwrap();
        let row = store.box_row(BoxId::new(1)).unwrap().unwrap();
        sink.lock().unwrap().push(row.intent);
    });
    start(&w).unwrap();
    let pid = std::process::id();
    assert_eq!(
        seen.lock().unwrap().first(),
        Some(&Some(Intent::Setup { pid }))
    );
    assert!(row(&w).intent.is_none());
}

#[test]
fn a_retry_in_the_kept_box_is_refused_while_another_op_holds_the_lock() {
    let (w, _) = world_with_setup([Ok(exit(1))]);
    start(&w).unwrap_err();
    let execs = w.driver.execs().len();
    let _held = hold(&w, "close");
    let err = start(&w).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: close in progress");
    assert_eq!(w.driver.execs().len(), execs);
    assert_eq!(row(&w).lifecycle, BoxLifecycle::SetupFailed);
}

#[test]
fn the_retry_in_the_kept_box_uses_the_providers_the_box_is_on_not_the_configs() {
    let yaml = |provider: &str| {
        let text = format!(
            "box: {{provider: {provider}}}\ndrivers: {{a: {{strip_env: [TOKEN_OF_A]}}, b: {{strip_env: [TOKEN_OF_B]}}}}"
        );
        moor_core::config::parse_user("config.yaml", &text).unwrap()
    };
    let mut w = super::world_with(
        FakeDriver::new().exec_results([Ok(exit(1)), Ok(exit(0)), Ok(exit(0))]),
        yaml("a"),
    );
    script_base_yaml(&w, Some(YAML));
    w.cx = w.cx.with_engine(Arc::new(FakeEngine::new()));
    start(&w).unwrap_err();
    let execs_before = w.driver.execs().len();

    let later = crate::cx::Cx::fake_with(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
        yaml("b"),
    )
    .with_forge(w.forge.clone())
    .with_engine(Arc::new(FakeEngine::new()));
    let again = run(
        &later,
        &StartReq {
            goal: Some("ship it".into()),
            ..start_req()
        },
    )
    .unwrap();
    assert!(again.setup.ran && again.setup.ok);
    let retried: Vec<String> = w
        .driver
        .execs()
        .iter()
        .skip(execs_before)
        .flat_map(|e| e.1.argv.clone())
        .collect();
    let text = retried.join("\n");
    assert!(text.contains("TOKEN_OF_A"), "{text}");
    assert!(!text.contains("TOKEN_OF_B"), "{text}");
}
