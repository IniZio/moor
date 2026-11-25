//! `ask` claims the caller's oldest ready idle place, serves its repos' rules, then sends the
//! question; with no idle place it starts one that holds the claim and the question.

use std::collections::BTreeSet;
use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::AskReq;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{LaunchPrompt, Steer};
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::user::{Email, Identity};
use moor_core::{BoxId, BoxPath, ErrorDetail, ErrorKind, StartReq};

use super::run;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::store::Store;
use crate::testing::agent;
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

/// A world whose requester is on the `agent_places` list as `ann@example.com`, whose forge
/// answers [`repo`].
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
    let roster = BTreeSet::from(["ann@example.com".to_owned()]);
    let cx = cx.with_env(|e| e.with_agent_place_users(roster));
    World {
        cx,
        engine,
        forge,
        proxy,
        _dir: dir,
    }
}

/// A start of place `name`.
fn place_req(name: &str) -> StartReq {
    StartReq {
        place: true,
        gates: None,
        placement: moor_core::decide::schedule::Placement::FirstOnly,
        cold: false,
        config: None,
        name: box_name(name),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
        task: None,
    }
}

/// Starts place `name`, whose agent then reads Idle.
fn place(w: &World, name: &str) -> BoxId {
    let v = crate::ops::start::run(&w.cx, &place_req(name)).unwrap();
    let id = BoxId::new(v.boxed.id);
    let launch = store(w)
        .live_launch(id)
        .unwrap()
        .expect("a place's agent launched");
    w.engine
        .set_probes(&launch.session_ref, agent::idle(Ts::from_millis(0)));
    id
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

/// The claim place `id` holds; none for a place no task claimed.
fn claim_of(w: &World, id: BoxId) -> Option<Claim> {
    match store(w).box_row(id).unwrap().unwrap().kind {
        BoxKind::Place(PlaceState::Claimed(c)) => Some(c),
        _ => None,
    }
}

/// The texts steered to the agents, in order.
fn steered(w: &World) -> Vec<String> {
    w.engine
        .steers()
        .into_iter()
        .filter_map(|(_, s)| match s {
            Steer::Text(t) => Some(t),
            _ => None,
        })
        .collect()
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
fn a_send_claims_the_oldest_ready_idle_place() {
    let w = world();
    let older = place(&w, "p1");
    let newer = place(&w, "p2");
    let v = run(&w.cx, &ask("Where is GET /items handled?")).unwrap();
    assert!(v.claimed);
    assert_eq!(v.box_name, "p1");
    let claim = claim_of(&w, older).unwrap();
    assert_eq!(
        (claim.repo, claim.prompt.as_str()),
        (repo(), "Where is GET /items handled?")
    );
    assert!(v.served_at.is_some() && v.steered_at.is_some());
    assert!(v.served_at <= v.steered_at);
    assert_eq!(steered(&w), [ASKED]);
    assert!(claim_of(&w, newer).is_none());
}

/// A claim of [`repo`] asking `prompt`.
fn claim(prompt: &str) -> Claim {
    Claim {
        repo: repo(),
        also: Vec::new(),
        prompt: prompt.to_owned(),
        at: 1,
        served: None,
        steered: None,
    }
}

#[test]
fn two_racing_sends_claim_two_places_or_one_goes_cold() {
    let w = world();
    let a = place(&w, "p1");
    let b = place(&w, "p2");
    let user = store(&w).box_row(a).unwrap().unwrap().user.unwrap();
    // Another send holds the oldest place while it claims it.
    let other = w.cx.turn(a, Cmd::Steer).unwrap();
    let first = run(&w.cx, &ask("one")).unwrap();
    assert!(store(&w)
        .claim_place(a, user, &claim("two"), "two")
        .unwrap());
    drop(other);
    let third = run(&w.cx, &ask("three")).unwrap();
    assert_eq!((first.box_name.as_str(), first.claimed), ("p2", true));
    assert!(!third.claimed, "{third:?}");
    assert!(third.box_name != "p1" && third.box_name != "p2");
    let prompts = [a, b].map(|id| claim_of(&w, id).unwrap().prompt);
    assert_eq!(prompts, ["two", "one"]);
}

#[test]
fn a_claimed_place_is_not_claimed_again() {
    let w = world();
    let id = place(&w, "p1");
    let user = store(&w).box_row(id).unwrap().unwrap().user.unwrap();
    assert!(store(&w)
        .claim_place(id, user, &claim("one"), "one")
        .unwrap());
    assert!(!store(&w)
        .claim_place(id, user, &claim("two"), "two")
        .unwrap());
    assert_eq!(claim_of(&w, id).unwrap().prompt, "one");
    let queued: Vec<String> = store(&w)
        .unsent(id)
        .unwrap()
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert_eq!(queued, ["one"]);
}

#[test]
fn the_prompt_waits_in_the_inbox_until_the_claim_is_served() {
    let w = world();
    let id = place(&w, "p1");
    let _ = w
        .proxy
        .clone()
        .with_ensure_error(ProxyHostError::Failed("no room".to_owned()));
    let err = run(&w.cx, &ask("Where is GET /items handled?")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed, "{}", err.error);
    assert!(steered(&w).is_empty());
    let claim = claim_of(&w, id).unwrap();
    assert_eq!((claim.served, claim.steered), (None, None));
    let queued: Vec<String> = store(&w)
        .unsent(id)
        .unwrap()
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert_eq!(queued, [ASKED]);
}

#[test]
fn a_crash_after_the_claim_row_serves_then_steers_once_on_the_next_delivery_pass() {
    let w = world();
    let id = place(&w, "p1");
    let user = store(&w).box_row(id).unwrap().unwrap().user.unwrap();
    let claim = claim("Where is GET /items handled?");
    // The ask's one write, then nothing: the process ended before it delivered.
    assert!(store(&w).claim_place(id, user, &claim, ASKED).unwrap());
    let backoff = crate::ops::InboxBackoff::default();
    crate::ops::inbox_pass(&w.cx, store(&w), &|| false, &backoff).unwrap();
    crate::ops::inbox_pass(&w.cx, store(&w), &|| false, &backoff).unwrap();
    assert_eq!(steered(&w), [ASKED]);
    let claim = claim_of(&w, id).unwrap();
    assert!(claim.served.is_some() && claim.served <= claim.steered);
    let last = specs(&w).pop().unwrap();
    assert_eq!(last.box_repo, Some(repo()));
}

#[test]
fn a_claimed_place_plans_rules_for_its_claim_repos() {
    let w = world();
    let other = ForgeRepo::new("o", "m");
    w.forge.answer(other.clone(), Ok(repo_at("abc123")));
    place(&w, "p1");
    let before = specs(&w).len();
    let req = AskReq {
        repos: vec![repo(), other.clone()],
        prompt: "q".to_owned(),
    };
    run(&w.cx, &req).unwrap();
    let served = specs(&w);
    let last = served.get(before..).unwrap().last().unwrap();
    assert_eq!(last.box_repo, Some(repo()));
    assert_eq!(last.other_repos, [other]);
    assert_eq!(steered(&w), ["Repos: o/n, o/m\n\nq"]);
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
    place(&w, "p1");
    let v = run(&w.cx, &ask("q")).unwrap();
    assert!(v.claimed);
}

#[test]
fn a_missing_secret_refuses_before_any_claim() {
    let mut w = world();
    let id = place(&w, "p1");
    let other = Identity::parse("local:2000").unwrap();
    store(&w)
        .enroll(&other, Some(&Email::parse("bob@example.com").unwrap()))
        .unwrap();
    let roster = BTreeSet::from(["ann@example.com".to_owned(), "bob@example.com".to_owned()]);
    w.cx =
        w.cx.with_requester(other)
            .with_env(|e| e.with_agent_place_users(roster));
    let err = run(&w.cx, &ask("q")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.detail.as_deref(), Some(&ErrorDetail::LoginRequired));
    assert!(claim_of(&w, id).is_none());
    assert!(steered(&w).is_empty());
}

#[test]
fn a_user_off_the_list_is_refused_before_any_claim() {
    let mut w = world();
    let id = place(&w, "p1");
    w.cx = w.cx.with_env(|e| e.with_agent_place_users(BTreeSet::new()));
    let err = run(&w.cx, &ask("q")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused, "{}", err.error);
    assert!(claim_of(&w, id).is_none());
}

#[test]
fn a_refusal_names_the_email_the_controller_holds_for_the_user() {
    let mut w = world();
    w.cx = w.cx.with_env(|e| e.with_agent_place_users(BTreeSet::new()));
    let err = run(&w.cx, &ask("q")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("agent_places"),
        "{}",
        err.error
    );
    assert!(
        err.error.to_string().contains("ann@example.com"),
        "{}",
        err.error
    );
}

#[test]
fn a_refusal_says_when_the_controller_holds_no_email_for_the_user() {
    let w = world();
    let other = Identity::parse("local:2000").unwrap();
    let (user, _) = store(&w).enroll(&other, None).unwrap();
    let err = crate::ops::place::on_roster(&w.cx, store(&w), user).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("no email"), "{}", err.error);
}

#[test]
fn no_idle_place_starts_a_place_with_the_claim_and_the_prompt() {
    let w = world();
    let v = run(&w.cx, &ask("Where is GET /items handled?")).unwrap();
    assert!(!v.claimed);
    let row = store(&w)
        .open_box_by_name(&box_name(&v.box_name))
        .unwrap()
        .unwrap();
    assert!(row.repo.is_none());
    let claim = claim_of(&w, row.id).unwrap();
    assert_eq!(claim.repo, repo());
    assert!(claim.served.is_some() && claim.steered.is_some());
    let launch = w.engine.launches().pop().unwrap();
    assert_eq!(launch.prompt, LaunchPrompt::Text(ASKED.to_owned()));
    assert!(launch.place, "a place launches with the place brief");
    let spec = specs(&w).pop().unwrap();
    assert_eq!(spec.box_repo, Some(repo()));
}
