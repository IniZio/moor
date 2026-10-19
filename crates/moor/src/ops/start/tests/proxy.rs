//! The per-task proxy is ensured through the proxy host before every agent launch.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{BoxHandle, BoxId, StartReq};

use super::relaunch::{gone, session};
use super::state::WORKING;
use super::{req, world, World};
use crate::ops::common::proxy::TAIL_LINES;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::FakeForge;
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

fn failure_step(w: &World, id: BoxId) -> (String, String, Option<String>) {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let f = row.launch_failure.unwrap();
    (f.step, f.message, f.tail)
}

#[test]
fn the_first_launch_ensures_the_proxy_once_and_hands_the_agent_its_address() {
    let (w, engine, fake) = host_world(FakeProxyHost::new());
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = BoxId::new(v.boxed.id);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(fake.calls().len(), 1);
    let spec = spec_of(&fake);
    assert_eq!(spec.box_id, id);
    assert_eq!(spec.box_handle, Some(BoxHandle::new("box-1")));
    assert_eq!(spec.host_cwd, PathBuf::from(row.worktree.unwrap()));
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
    let id = BoxId::new(v.boxed.id);
    assert!(!v.agent.launched);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(engine.launches().is_empty());
    let (step, message, tail) = failure_step(&w, id);
    assert_eq!(step, "proxy");
    assert_eq!(message, "boom");
    assert_eq!(tail.as_deref(), Some("tail"));
}

#[test]
fn a_missing_default_credential_fails_the_proxy_step_naming_the_key() {
    let mut w = super::world_with(
        FakeDriver::new(),
        std::path::Path::new("/repo"),
        moor_core::config::UserConfig::default(),
    );
    w.cx = w.cx.with_proxy_host(Arc::new(FakeProxyHost::new()));
    let v = run(&w.cx, &goal_req()).unwrap();
    let (step, message, _) = failure_step(&w, BoxId::new(v.boxed.id));
    assert_eq!(step, "proxy");
    assert!(
        message.contains(
            "the default claude rule needs credentials.CLAUDE_CODE_OAUTH_TOKEN; add it under credentials:"
        ),
        "{message}"
    );
}

#[test]
fn only_the_last_lines_of_a_long_proxy_failure_are_kept() {
    let mut text = String::from("boom");
    text.extend((1..=25).map(|n| format!("\nline {n}")));
    let fake = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed(text));
    let (w, _, _) = host_world(fake);
    let v = run(&w.cx, &goal_req()).unwrap();
    let (_, _, tail) = failure_step(&w, BoxId::new(v.boxed.id));
    let tail = tail.unwrap();
    assert_eq!(tail.lines().count(), TAIL_LINES);
    assert_eq!(tail.lines().last(), Some("line 25"));
}

#[test]
fn the_nonce_stays_out_of_every_record_and_the_spec() {
    let (w, engine, fake) = host_world(FakeProxyHost::new());
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = BoxId::new(v.boxed.id);
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

fn spec_of(fake: &FakeProxyHost) -> moor_core::seam::proxy_host::ProxySpec {
    fake.calls()
        .into_iter()
        .find_map(|c| match c {
            ProxyHostCall::Ensure(spec) => Some(spec),
            _ => None,
        })
        .unwrap()
}

fn env_of<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn the_spec_carries_the_task_repo_and_resolved_rules() {
    let (mut w, engine, fake) = host_world(FakeProxyHost::new());
    w.cx = w.cx.with_forge(Arc::new(
        FakeForge::new().with_repo("git@github.com:o/r.git", ForgeRepo::new("o", "r")),
    ));
    w.proc.on(
        Tool::Git,
        &["remote", "get-url", "origin"],
        Reply::Ok("git@github.com:o/r.git\n".into()),
    );
    run(&w.cx, &goal_req()).unwrap();
    let spec = spec_of(&fake);
    assert_eq!(spec.box_repo, Some(ForgeRepo::new("o", "r")));
    let names: Vec<&str> = spec.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["claude", "github-api", "github-git", "github-uploads"]
    );
    let api = spec.rules.iter().find(|r| r.name == "github-api").unwrap();
    assert_eq!(api.box_env.get("GH_REPO").map(String::as_str), Some("o/r"));
    let launches = engine.launches();
    let addr = launches.first().unwrap().proxy.as_ref().unwrap();
    assert_eq!(env_of(&addr.box_env, "GH_REPO"), Some("o/r"));
    assert_eq!(
        env_of(&addr.box_env, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some("moor-proxy")
    );
    let nonce = String::from_utf8(FAKE_NONCE.to_vec()).unwrap();
    assert!(!format!("{:?}", addr.box_env).contains(&nonce));
}

#[test]
fn without_an_origin_only_the_rules_that_need_no_repo_remain() {
    let (w, engine, fake) = host_world(FakeProxyHost::new());
    run(&w.cx, &goal_req()).unwrap();
    let spec = spec_of(&fake);
    assert_eq!(spec.box_repo, None);
    let names: Vec<&str> = spec.rules.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"claude"), "{names:?}");
    assert!(!names.contains(&"github-api"), "{names:?}");
    let launches = engine.launches();
    let addr = launches.first().unwrap().proxy.as_ref().unwrap();
    assert_eq!(env_of(&addr.box_env, "GH_REPO"), None);
    assert_eq!(
        env_of(&addr.box_env, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some("moor-proxy")
    );
}

/// Starts `demo` in a repo whose `.moor.yaml` is `yaml`, with the origin `o/r`; returns the first launch's env.
fn repo_env_launch(yaml: &str) -> (World, Vec<(String, String)>) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let engine = Arc::new(FakeEngine::new());
    let mut w = super::world_with(
        FakeDriver::new(),
        repo.path(),
        crate::testing::cx::fake_config(),
    );
    w.cx = w.cx.with_engine(engine.clone()).with_forge(Arc::new(
        FakeForge::new().with_repo("git@github.com:o/r.git", ForgeRepo::new("o", "r")),
    ));
    w.proc.on(
        Tool::Git,
        &["remote", "get-url", "origin"],
        Reply::Ok("git@github.com:o/r.git\n".into()),
    );
    run(&w.cx, &goal_req()).unwrap();
    let env = engine
        .launches()
        .first()
        .unwrap()
        .proxy
        .as_ref()
        .unwrap()
        .box_env
        .clone();
    (w, env)
}

#[test]
fn a_rule_env_value_wins_over_the_repo_env_of_the_same_name() {
    let (_w, env) = repo_env_launch("env:\n  GH_REPO: other/repo\n");
    assert_eq!(env_of(&env, "GH_REPO"), Some("o/r"));
    assert_eq!(env.iter().filter(|(k, _)| k == "GH_REPO").count(), 1);
}

#[test]
fn repo_env_never_sets_proxy_managed_or_loader_names() {
    let names = [
        "HTTPS_PROXY",
        "no_proxy",
        "SSL_CERT_FILE",
        "LD_PRELOAD",
        "HOME",
        "NODE_OPTIONS",
    ];
    let yaml = names.map(|n| format!("  {n}: x\n")).concat();
    let (w, env) = repo_env_launch(&format!("env:\n  KEEP: one\n{yaml}"));
    assert_eq!(env_of(&env, "KEEP"), Some("one"));
    for n in names {
        assert_eq!(env_of(&env, n), None, "{n}");
    }
    let dropped = (w.cx.take_warnings().iter())
        .filter(|x| x.code == "agent_env_dropped")
        .count();
    assert_eq!(dropped, names.len());
}

#[test]
fn repo_env_reaches_the_agent_and_credential_looking_names_are_dropped() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        "env:\n  APP_MODE: dev\n  DEPLOY_TOKEN: real\n  api_key: real\n",
    )
    .unwrap();
    let engine = Arc::new(FakeEngine::new());
    let mut w = super::world_with(
        FakeDriver::new(),
        repo.path(),
        crate::testing::cx::fake_config(),
    );
    w.cx = w.cx.with_engine(engine.clone());
    run(&w.cx, &goal_req()).unwrap();
    let launches = engine.launches();
    let addr = launches.first().unwrap().proxy.as_ref().unwrap();
    assert_eq!(env_of(&addr.box_env, "APP_MODE"), Some("dev"));
    assert_eq!(env_of(&addr.box_env, "DEPLOY_TOKEN"), None);
    assert_eq!(env_of(&addr.box_env, "api_key"), None);
    let dropped =
        w.cx.take_warnings()
            .iter()
            .filter(|x| x.code == "agent_env_dropped")
            .count();
    assert_eq!(dropped, 2);
}

/// Starts `demo` with setup lines and the repo env `yaml`; returns the world and the agent launch env.
fn setup_env_launch(yaml: &str) -> (World, Vec<(String, String)>) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let engine = Arc::new(FakeEngine::new());
    let mut w = super::world_with(
        FakeDriver::new(),
        repo.path(),
        crate::testing::cx::fake_config(),
    );
    w.cx = w.cx.with_engine(engine.clone());
    super::moor_yaml::script_base_yaml(&w, Some(yaml));
    run(&w.cx, &goal_req()).unwrap();
    let env = engine
        .launches()
        .first()
        .unwrap()
        .proxy
        .as_ref()
        .unwrap()
        .box_env
        .clone();
    (w, env)
}

const SETUP_ENV_YAML: &str =
    "setup: [a, b]\nenv:\n  A: a\n  STRIPE_TEST_KEY: sk\n  NODE_OPTIONS: --x\n";

#[test]
fn a_start_with_setup_warns_once_per_dropped_env_name() {
    let (w, _env) = setup_env_launch(SETUP_ENV_YAML);
    let warnings = w.cx.take_warnings();
    let count = |name: &str| {
        (warnings.iter())
            .filter(|x| x.code == "agent_env_dropped" && x.message.contains(name))
            .count()
    };
    assert_eq!(count("STRIPE_TEST_KEY"), 1, "{warnings:?}");
    assert_eq!(count("NODE_OPTIONS"), 1, "{warnings:?}");
}

#[test]
fn the_agent_launch_env_drops_reserved_and_credential_names_but_keeps_the_rest() {
    let (_w, env) = setup_env_launch(SETUP_ENV_YAML);
    assert_eq!(env_of(&env, "A"), Some("a"));
    assert_eq!(env_of(&env, "NODE_OPTIONS"), None);
    assert_eq!(env_of(&env, "STRIPE_TEST_KEY"), None);
}
