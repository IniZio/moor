//! `ask` starts a place that holds the claim and the question, for anyone signed in, and plans
//! the claim's repos' rules before its agent reads the question.

use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::AskReq;
use moor_core::seam::engine::LaunchPrompt;
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::{Email, Identity};
use moor_core::{BoxId, BoxPath, ErrorDetail, ErrorKind};

use super::run;
use crate::cx::Cx;
use crate::store::Store;
use crate::testing::box_name;
use crate::testing::cx::requester;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};

struct World {
    cx: Cx,
    engine: Arc<FakeEngine>,
    forge: Arc<FakeForge>,
    proxy: FakeProxyHost,
    _dir: tempfile::TempDir,
}

/// The task's repo.
fn repo() -> ForgeRepo {
    ForgeRepo::new("o", "n")
}

/// A world whose requester is enrolled as `ann@example.com`, whose forge answers [`repo`].
fn world() -> World {
    let proxy = FakeProxyHost::new();
    let dir = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=4).fold(FakeDriver::new(), |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
    let forge = Arc::new(FakeForge::new());
    forge.answer(repo(), Ok(repo_at("abc123")));
    let engine = Arc::new(FakeEngine::new());
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake_with(dir.path(), driver, proc, UserConfig::default())
        .with_forge(forge.clone())
        .with_engine(engine.clone())
        .with_proxy_host(Arc::new(proxy.clone()));
    cx.store()
        .unwrap()
        .enroll(
            &requester(),
            Some(&Email::parse("ann@example.com").unwrap()),
        )
        .unwrap();
    World {
        cx,
        engine,
        forge,
        proxy,
        _dir: dir,
    }
}

fn store(w: &World) -> &Store {
    w.cx.store().unwrap()
}

fn ask(prompt: &str) -> AskReq {
    AskReq {
        repos: vec![repo()],
        prompt: prompt.to_owned(),
    }
}

/// The claim place `id` holds; none for a Box that holds none.
fn claim_of(w: &World, id: BoxId) -> Option<Claim> {
    match store(w).box_row(id).unwrap().unwrap().kind {
        BoxKind::Place(PlaceState::Claimed(c)) => Some(c),
        _ => None,
    }
}

/// The Proxy specs stored, in order.
fn specs(w: &World) -> Vec<moor_core::seam::proxy_host::ProxySpec> {
    w.proxy
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            ProxyHostCall::Ensure(s) => Some(s),
            _ => None,
        })
        .collect()
}

const ASKED: &str = "Repos: o/n\n\nWhere is GET /items handled?";

#[test]
fn an_ask_starts_a_place_with_the_claim_and_the_prompt() {
    let w = world();
    let v = run(&w.cx, &ask("Where is GET /items handled?")).unwrap();
    let row = store(&w)
        .open_box_by_name(&box_name(&v.box_name))
        .unwrap()
        .unwrap();
    assert!(row.repo.is_none());
    let claim = claim_of(&w, row.id).unwrap();
    assert_eq!(claim.repo, repo());
    assert!(claim.served.is_some() && claim.steered.is_some());
    assert!(v.served_at.is_some() && v.served_at <= v.steered_at);
    let launch = w.engine.launches().pop().unwrap();
    assert_eq!(launch.prompt, LaunchPrompt::Text(ASKED.to_owned()));
    assert!(launch.place, "a place launches with the place brief");
    let spec = specs(&w).pop().unwrap();
    assert_eq!(spec.box_repo, Some(repo()));
}

#[test]
fn a_place_plans_rules_for_its_claim_repos() {
    let w = world();
    let other = ForgeRepo::new("o", "m");
    w.forge.answer(other.clone(), Ok(repo_at("abc123")));
    let req = AskReq {
        repos: vec![repo(), other.clone()],
        prompt: "q".to_owned(),
    };
    run(&w.cx, &req).unwrap();
    let last = specs(&w).pop().unwrap();
    assert_eq!(last.box_repo, Some(repo()));
    assert_eq!(last.other_repos, [other]);
    let launch = w.engine.launches().pop().unwrap();
    assert_eq!(
        launch.prompt,
        LaunchPrompt::Text("Repos: o/n, o/m\n\nq".to_owned())
    );
}

#[test]
fn a_read_only_repo_is_allowed() {
    let w = world();
    w.forge.answer(
        repo(),
        Ok(moor_core::seam::forge::RepoAt {
            can_push: false,
            ..repo_at("abc123")
        }),
    );
    assert!(run(&w.cx, &ask("q")).is_ok());
}

#[test]
fn a_missing_secret_refuses_before_any_place_starts() {
    let mut w = world();
    let other = Identity::parse("local:2000").unwrap();
    store(&w)
        .enroll(&other, Some(&Email::parse("bob@example.com").unwrap()))
        .unwrap();
    w.cx = w.cx.with_requester(other);
    let err = run(&w.cx, &ask("q")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.detail.as_deref(), Some(&ErrorDetail::LoginRequired));
    assert!(store(&w).open_boxes().unwrap().is_empty());
    assert!(w.engine.launches().is_empty());
}
