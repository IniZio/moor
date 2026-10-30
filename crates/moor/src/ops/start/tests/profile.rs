//! The requester's agent profile, resolved at start and kept on the Box.

use moor_core::name::ProfileSource;
use moor_core::user::UserId;
use moor_core::Sha;

use super::setup::{kinds, warning_codes};
use super::*;
use crate::testing::engine::FakeEngine;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use moor_core::seam::driver::ExecOutcome;

pub(super) const HEAD: &str = "1111111111111111111111111111111111111111";
const PIN: &str = "2222222222222222222222222222222222222222";

fn profile_repo() -> ForgeRepo {
    ForgeRepo::new("ann", "profile")
}

/// The user the world's starts act for, learned from a throwaway Box.
fn requester(w: &World) -> UserId {
    let v = run(&w.cx, &agentless_req("warmup")).unwrap();
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    row.unwrap().unwrap().user.unwrap()
}

pub(super) fn set_profile(w: &World, git_ref: Option<&str>) {
    let user = requester(w);
    let source = ProfileSource {
        repo: profile_repo(),
        git_ref: git_ref.map(str::to_owned),
    };
    w.cx.store()
        .unwrap()
        .set_user_profile(user, &source)
        .unwrap();
}

pub(super) fn world_answering(at: Option<&str>, sha: &str, manifest: Option<&str>) -> World {
    answering(FakeDriver::new(), at, sha, manifest)
}

pub(super) fn answering(
    driver: FakeDriver,
    at: Option<&str>,
    sha: &str,
    manifest: Option<&str>,
) -> World {
    let mut w = world(driver);
    let forge = FakeForge::new().with_file(
        profile_repo(),
        at,
        "moor-profile.yaml",
        Ok((Sha::parse(sha).unwrap(), manifest.map(str::to_owned))),
    );
    w.forge = Arc::new(forge);
    w.cx = w.cx.with_forge(w.forge.clone());
    w
}

fn started(w: &World) -> BoxId {
    BoxId::new(run(&w.cx, &agentless_req("demo")).unwrap().boxed.id)
}

fn profile_events(w: &World, id: BoxId) -> Vec<(String, serde_json::Value)> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind.starts_with("profile_"))
        .map(|e| (e.kind, e.payload))
        .collect()
}

#[test]
fn a_person_with_no_profile_gets_no_reads_and_no_events() {
    let w = world(FakeDriver::new());
    let id = started(&w);
    assert!(w.forge.files_asked().is_empty());
    assert!(profile_events(&w, id).is_empty());
    assert_eq!(w.cx.store().unwrap().box_profile(id).unwrap(), None);
}

#[test]
fn an_unpinned_profile_records_the_default_branch_head() {
    let w = world_answering(None, HEAD, None);
    set_profile(&w, None);
    let id = started(&w);
    let got = w.cx.store().unwrap().box_profile(id).unwrap().unwrap();
    assert_eq!(got.sha.as_str(), HEAD);
    assert_eq!(got.reads, [profile_repo()]);
    let events = profile_events(&w, id);
    assert_eq!(events.len(), 1);
    assert_eq!(events.first().unwrap().0, "profile_resolved");
    assert_eq!(
        events
            .first()
            .unwrap()
            .1
            .get("repo")
            .and_then(serde_json::Value::as_str),
        Some("ann/profile")
    );
    assert_eq!(
        events
            .first()
            .unwrap()
            .1
            .get("sha")
            .and_then(serde_json::Value::as_str),
        Some(HEAD)
    );
}

#[test]
fn a_pinned_profile_is_read_at_its_pin() {
    let w = world_answering(Some(PIN), PIN, Some("claude: {model: opus}\n"));
    set_profile(&w, Some(PIN));
    let id = started(&w);
    let got = w.cx.store().unwrap().box_profile(id).unwrap().unwrap();
    assert_eq!(got.sha.as_str(), PIN);
    assert_eq!(got.manifest.as_deref(), Some("claude: {model: opus}\n"));
}

#[test]
fn a_bad_manifest_is_reported_and_the_box_still_starts() {
    let w = world_answering(None, HEAD, Some("nope: 1\n"));
    set_profile(&w, None);
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    let events = profile_events(&w, BoxId::new(v.boxed.id));
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["profile_resolved", "profile_failed"]);
    assert_eq!(
        events
            .get(1)
            .unwrap()
            .1
            .get("step")
            .and_then(serde_json::Value::as_str),
        Some("manifest")
    );
    assert!(warning_codes(&w).contains(&"profile_failed".to_owned()));
}

#[test]
fn an_unresolvable_profile_is_reported_and_the_box_still_starts() {
    let w = world(FakeDriver::new());
    set_profile(&w, Some("gone"));
    let id = started(&w);
    let events = profile_events(&w, id);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events
            .first()
            .unwrap()
            .1
            .get("step")
            .and_then(serde_json::Value::as_str),
        Some("resolve")
    );
    assert!(!kinds(&w, id).is_empty());
}

#[test]
fn marketplaces_and_their_plugin_sources_join_the_reads() {
    let (w, id) = with_marketplace("tools");
    assert!(profile_events(&w, id)
        .iter()
        .all(|(k, _)| k == "profile_resolved"));
    let got = w.cx.store().unwrap().box_profile(id).unwrap().unwrap();
    assert_eq!(
        got.reads,
        [
            profile_repo(),
            ForgeRepo::new("acme", "tools"),
            ForgeRepo::new("acme", "lint")
        ]
    );
}

#[test]
fn a_marketplace_named_unlike_its_key_drops_its_plugins() {
    let (w, id) = with_marketplace("kit");
    let events = profile_events(&w, id);
    assert_eq!(events.get(1).unwrap().0, "profile_failed");
    assert_eq!(
        events
            .get(1)
            .unwrap()
            .1
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some("marketplace tools is named kit")
    );
    let got = w.cx.store().unwrap().box_profile(id).unwrap().unwrap();
    assert_eq!(got.reads, [profile_repo(), ForgeRepo::new("acme", "tools")]);
}

/// Starts a Box whose profile names `lint@tools`, with the `tools` marketplace calling itself
/// `named`.
fn with_marketplace(named: &str) -> (World, BoxId) {
    let manifest = "claude:\n  marketplaces: {tools: https://github.com/acme/tools}\n  plugins: [lint@tools]\n";
    let market = format!(
        r#"{{"name":"{named}","plugins":[{{"name":"lint","source":{{"source":"github","repo":"acme/lint"}}}}]}}"#
    );
    let mut w = world_answering(None, HEAD, Some(manifest));
    let forge = FakeForge::new()
        .with_file(
            profile_repo(),
            None,
            "moor-profile.yaml",
            Ok((Sha::parse(HEAD).unwrap(), Some(manifest.to_owned()))),
        )
        .with_file(
            ForgeRepo::new("acme", "tools"),
            None,
            ".claude-plugin/marketplace.json",
            Ok((Sha::parse(PIN).unwrap(), Some(market))),
        );
    w.forge = Arc::new(forge);
    w.cx = w.cx.with_forge(w.forge.clone());
    set_profile(&w, None);
    let id = started(&w);
    (w, id)
}

fn fetches(w: &World) -> Vec<String> {
    w.driver
        .execs()
        .into_iter()
        .map(|(_, spec, _)| spec.argv.join(" "))
        .filter(|argv| argv.contains("git fetch"))
        .collect()
}

fn with_proxy(mut w: World) -> (World, FakeProxyHost) {
    let fake = FakeProxyHost::new();
    w.cx = w.cx.with_proxy_host(Arc::new(fake.clone()));
    (w, fake)
}

fn git_paths(fake: &FakeProxyHost) -> Vec<String> {
    let spec = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            ProxyHostCall::Ensure(spec) => Some(spec),
            _ => None,
        })
        .next_back()
        .unwrap();
    let git = spec.rules.iter().find(|r| r.name == "github-git").unwrap();
    git.endpoint_rules
        .iter()
        .map(|e| format!("{} {}", e.method, e.path))
        .collect()
}

#[test]
fn the_box_fetches_exactly_the_resolved_commit() {
    let w = world_answering(None, HEAD, None);
    set_profile(&w, None);
    started(&w);
    let got = fetches(&w);
    assert_eq!(got.len(), 1, "{got:?}");
    let script = got.first().unwrap();
    assert!(
        script.contains("'https://github.com/ann/profile.git'"),
        "{script}"
    );
    assert!(
        script.contains(&format!("--depth 1 origin '{HEAD}'")),
        "{script}"
    );
}

#[test]
fn a_box_without_a_profile_fetches_nothing() {
    let w = world(FakeDriver::new());
    started(&w);
    assert!(fetches(&w).is_empty());
}

#[test]
fn a_failed_fetch_is_reported_and_the_box_still_starts() {
    let failing =
        FakeDriver::new().exec_ends_when(".moor-profile/person", ExecOutcome::Exited { code: 128 });
    let w = answering(failing, None, HEAD, None);
    set_profile(&w, None);
    w.cx.take_warnings();
    let v = run(&w.cx, &req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    let events = profile_events(&w, BoxId::new(v.boxed.id));
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["profile_resolved", "profile_failed"]);
    let step = events.last().unwrap().1.get("step").cloned();
    assert_eq!(step, Some(serde_json::json!("fetch")));
    assert!(warning_codes(&w).contains(&"profile_failed".to_owned()));
}

#[test]
fn the_proxy_lets_the_box_read_but_never_push_its_profile() {
    let (w, fake) = with_proxy(world_answering(None, HEAD, None));
    set_profile(&w, None);
    run(&w.cx, &req("demo")).unwrap();
    let paths = git_paths(&fake);
    assert!(
        paths.contains(&"GET /ann/profile.git/info/refs".to_owned()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"POST /ann/profile.git/git-upload-pack".to_owned()),
        "{paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("ann/profile.git/git-receive-pack")),
        "{paths:?}"
    );
}

#[test]
fn a_relaunch_reuses_the_stored_commit_without_reading_the_forge() {
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Ok(super::relaunch::session("p-1")))
            .launch_result(Ok(super::relaunch::session("p-2"))),
    );
    let (mut w, fake) = with_proxy(world_answering(None, HEAD, None));
    w.cx = w.cx.with_engine(engine.clone());
    set_profile(&w, None);
    let first = run(&w.cx, &req("demo")).unwrap();
    let asked = w.forge.files_asked().len();
    super::relaunch::gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    assert_eq!(engine.launches().len(), 2);
    assert_eq!(w.forge.files_asked().len(), asked);
    let id = BoxId::new(first.boxed.id);
    let got = w.cx.store().unwrap().box_profile(id).unwrap().unwrap();
    assert_eq!(got.sha.as_str(), HEAD);
    assert!(git_paths(&fake).contains(&"POST /ann/profile.git/git-upload-pack".to_owned()));
}
