//! The per-task proxy is ensured through the proxy host before every agent launch.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::{ProxyHostError, ProxySpec};
use moor_core::{BoxHandle, StartReq, TaskId};

use super::relaunch::{gone, session};
use super::{req, world, World};
use crate::ops::common::proxy::TAIL_LINES;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall, FAKE_BOX_PORT, FAKE_NONCE};

fn goal_req() -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    }
}

/// A world whose engine records launches and whose proxy host is the returned fake.
fn host_world(fake: FakeProxyHost) -> (World, Arc<FakeEngine>, FakeProxyHost) {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world(FakeDriver::new());
    w.cx =
        w.cx.with_engine(engine.clone())
            .with_proxy_host(Arc::new(fake.clone()));
    (w, engine, fake)
}

fn engine_world() -> (World, Arc<FakeEngine>) {
    let (w, engine, _) = host_world(FakeProxyHost::new());
    (w, engine)
}

fn failure_step(w: &World, id: TaskId) -> (String, String, Option<String>) {
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    let f = row.launch_failure.unwrap();
    (f.step, f.message, f.tail)
}

#[test]
fn the_first_launch_ensures_the_proxy_once_and_hands_the_agent_its_address() {
    let (w, engine, fake) = host_world(FakeProxyHost::new());
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    let spec = ProxySpec {
        task: id,
        box_handle: Some(BoxHandle::new("box-1")),
        host_cwd: PathBuf::from(row.worktree.unwrap()),
        task_repo: None,
    };
    assert_eq!(fake.calls(), [ProxyHostCall::Ensure(spec)]);
    let launches = engine.launches();
    let addr = launches.first().unwrap().proxy.as_ref().unwrap();
    assert_eq!(addr.box_port, FAKE_BOX_PORT);
    assert_eq!(addr.nonce.expose(), FAKE_NONCE);
}

#[test]
fn the_launch_carries_the_repo_origin_when_there_is_one() {
    let (w, engine) = engine_world();
    w.proc.on(
        Tool::Git,
        &["remote", "get-url", "origin"],
        Reply::Ok("git@github.com:o/r.git\n".into()),
    );
    run(&w.cx, &goal_req()).unwrap();
    let launches = engine.launches();
    let origin = launches.first().unwrap().origin.as_deref();
    assert_eq!(origin, Some("git@github.com:o/r.git"));
}

#[test]
fn the_launch_has_no_origin_without_a_remote() {
    let (w, engine) = engine_world();
    run(&w.cx, &goal_req()).unwrap();
    assert_eq!(engine.launches().first().unwrap().origin, None);
}

#[test]
fn a_relaunch_ensures_the_proxy_again() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(session("p-1")))
            .launch_result(Ok(session("p-2"))),
    );
    let fake = FakeProxyHost::new();
    let mut w = world(FakeDriver::new());
    w.cx =
        w.cx.with_engine(engine.clone())
            .with_proxy_host(Arc::new(fake.clone()));
    run(&w.cx, &goal_req()).unwrap();
    gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    let launches = engine.launches();
    assert_eq!(launches.len(), 2);
    let ensures = fake
        .calls()
        .into_iter()
        .filter(|c| matches!(c, ProxyHostCall::Ensure(_)))
        .count();
    assert_eq!(ensures, 2);
    let [first, second] = [0, 1].map(|n| launches.get(n).unwrap().proxy.clone().unwrap());
    assert_eq!(first.box_port, second.box_port);
    assert_eq!(first.nonce.expose(), second.nonce.expose());
}

#[test]
fn a_proxy_that_cannot_be_ensured_fails_the_launch_at_the_proxy_step() {
    let fake = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom\ntail".into()));
    let (w, engine, _) = host_world(fake);
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    assert!(!v.agent.launched);
    assert_eq!(v.task.state, moor_core::domain::task::TaskState::Working);
    assert!(engine.launches().is_empty());
    let (step, message, tail) = failure_step(&w, id);
    assert_eq!(step, "proxy");
    assert_eq!(message, "boom");
    assert_eq!(tail.as_deref(), Some("tail"));
}

#[test]
fn only_the_last_lines_of_a_long_proxy_failure_are_kept() {
    let mut text = String::from("boom");
    text.extend((1..=25).map(|n| format!("\nline {n}")));
    let fake = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed(text));
    let (w, _, _) = host_world(fake);
    let v = run(&w.cx, &goal_req()).unwrap();
    let (_, _, tail) = failure_step(&w, TaskId::new(v.task.id));
    let tail = tail.unwrap();
    assert_eq!(tail.lines().count(), TAIL_LINES);
    assert_eq!(tail.lines().last(), Some("line 25"));
}

#[test]
fn the_nonce_stays_out_of_every_record_and_the_spec() {
    let (w, engine, fake) = host_world(FakeProxyHost::new());
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    let nonce = String::from_utf8(FAKE_NONCE.to_vec()).unwrap();
    let mut seen = vec![format!("{:?}", fake.calls())];
    for call in w.proc.calls() {
        seen.push(format!("{:?} {:?}", call.args, call.env));
    }
    for e in w.cx.store().unwrap().events(id, 1000).unwrap() {
        seen.push(format!("{} {}", e.kind, e.payload));
    }
    seen.push(format!("{:?}", engine.launches()));
    for text in seen {
        assert!(!text.contains(&nonce), "{text}");
    }
}
