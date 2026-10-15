//! Tests for refusing an agent relaunch when the worktree `.moor.yaml` differs from the base.

use std::path::Path;
use std::sync::Arc;

use moor_core::{ErrorKind, StartReq, TaskId};

use super::moor_yaml::script_base_yaml;
use super::relaunch::{gone, session, started};
use super::setup::{exit, start_req};
use super::{req, world_at, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const BASE: &str = "setup: [a]\nenv:\n  A: \"1\"\n";
const CHANGED: &str = "setup: [a]\nenv:\n  A: \"2\"\n";

/// Leaves `yaml` as the `.moor.yaml` in the worktree of task `id`.
fn write_worktree_yaml(w: &World, id: TaskId, yaml: &str) {
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), yaml).unwrap();
}

/// A started Task whose agent is gone, over a base holding `BASE`, with `yaml` in its worktree.
fn gone_with(yaml: &str) -> (World, Arc<FakeEngine>, TaskId) {
    let (w, engine, id) = started();
    script_base_yaml(&w, Some(BASE));
    write_worktree_yaml(&w, id, yaml);
    gone(&w);
    (w, engine, id)
}

fn trusted() -> StartReq {
    StartReq {
        trust_moor_yaml: true,
        ..req("demo")
    }
}

fn trusted_warnings(w: &World) -> usize {
    w.cx.take_warnings()
        .iter()
        .filter(|x| x.code == "moor_yaml_trusted")
        .count()
}

#[test]
fn a_changed_moor_yaml_refuses_a_relaunch_and_launches_no_agent() {
    let (w, engine, _) = gone_with(CHANGED);
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("env"), "{}", err.error);
    assert!(err.hints.iter().any(|h| h.contains("--trust-moor-yaml")));
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_changed_moor_yaml_relaunches_when_trusted_and_warns_once() {
    let (w, engine, _) = gone_with(CHANGED);
    drop(w.cx.take_warnings());
    let v = run(&w.cx, &trusted()).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(trusted_warnings(&w), 1);
}

#[test]
fn an_unchanged_moor_yaml_relaunches_without_a_warning() {
    let (w, engine, _) = gone_with(BASE);
    drop(w.cx.take_warnings());
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(trusted_warnings(&w), 0);
}

#[test]
fn a_start_that_re_runs_setup_and_launches_warns_once() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), BASE).unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let mut w = world_at(FakeDriver::new().exec_results([Ok(exit(1))]), repo.path());
    w.cx = w.cx.with_engine(engine.clone());
    script_base_yaml(&w, Some(BASE));
    let first = run(
        &w.cx,
        &StartReq {
            goal: Some("ship it".into()),
            ..start_req()
        },
    )
    .unwrap();
    assert!(first.setup.ran && !first.setup.ok);
    assert_eq!(engine.launches().len(), 1);
    gone(&w);
    write_worktree_yaml(&w, TaskId::new(first.task.id), CHANGED);
    script_base_yaml(&w, Some(BASE));
    drop(w.cx.take_warnings());
    let again = run(&w.cx, &trusted()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert!(again.agent.launched);
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(trusted_warnings(&w), 1);
}
