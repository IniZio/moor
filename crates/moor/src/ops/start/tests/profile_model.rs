//! Which model the agent launches with once the Box carries a profile.

use super::profile::{set_profile, world_answering, HEAD};
use super::relaunch::{gone, session};
use super::*;
use crate::testing::engine::FakeEngine;

/// A world whose user config asks for `sonnet`, whose requester's profile asks for `opus`, and
/// whose launches the returned engine records.
fn model_world(manifest: Option<&str>) -> (World, Arc<FakeEngine>) {
    model_world_with(manifest, FakeEngine::new())
}

fn model_world_with(manifest: Option<&str>, engine: FakeEngine) -> (World, Arc<FakeEngine>) {
    let mut w = world_answering(None, HEAD, manifest);
    set_profile(&w, None);
    let mut config = w.cx.config().clone();
    config.agent.model = Some("sonnet".into());
    let engine = Arc::new(engine);
    w.cx = w.cx.with_config(config).with_engine(engine.clone());
    (w, engine)
}

fn launched_model(engine: &FakeEngine) -> Option<String> {
    let launches = engine.launches();
    assert_eq!(launches.len(), 1);
    launches.into_iter().next().and_then(|l| l.model)
}

#[test]
fn the_profile_model_beats_the_user_config() {
    let (w, engine) = model_world(Some("claude: {model: opus}\n"));
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(launched_model(&engine).as_deref(), Some("opus"));
    assert!(engine.launches().iter().all(|l| l.profile.is_some()));
}

#[test]
fn the_model_flag_beats_the_profile_model() {
    let (w, engine) = model_world(Some("claude: {model: opus}\n"));
    let flagged = StartReq {
        model: Some("haiku".into()),
        ..req("demo")
    };
    run(&w.cx, &flagged).unwrap();
    assert_eq!(launched_model(&engine).as_deref(), Some("haiku"));
}

#[test]
fn a_profile_without_a_model_keeps_the_user_config_model() {
    let (w, engine) = model_world(None);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(launched_model(&engine).as_deref(), Some("sonnet"));
}

/// The models of a start and then a relaunch, the relaunch sent as `again`.
fn relaunched_models(again: &StartReq) -> Vec<Option<String>> {
    let engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .launch_result(Ok(session("p-2")));
    let (w, engine) = model_world_with(Some("claude: {model: opus}\n"), engine);
    run(&w.cx, &req("demo")).unwrap();
    gone(&w);
    run(&w.cx, again).unwrap();
    engine.launches().into_iter().map(|l| l.model).collect()
}

#[test]
fn a_relaunch_keeps_the_stored_profile_model() {
    let got = relaunched_models(&req("demo"));
    assert_eq!(got, [Some("opus".to_owned()), Some("opus".to_owned())]);
}

#[test]
fn a_relaunch_with_a_model_flag_uses_the_flag() {
    let flagged = StartReq {
        model: Some("haiku".into()),
        ..req("demo")
    };
    let got = relaunched_models(&flagged);
    assert_eq!(got, [Some("opus".to_owned()), Some("haiku".to_owned())]);
}
