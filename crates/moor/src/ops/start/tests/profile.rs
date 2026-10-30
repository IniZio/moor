//! The requester's agent profile, resolved at start and kept on the Box.

use moor_core::name::ProfileSource;
use moor_core::user::UserId;
use moor_core::Sha;

use super::setup::{kinds, warning_codes};
use super::*;

const HEAD: &str = "1111111111111111111111111111111111111111";
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

fn set_profile(w: &World, git_ref: Option<&str>) {
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

fn world_answering(at: Option<&str>, sha: &str, manifest: Option<&str>) -> World {
    let mut w = world(FakeDriver::new());
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
        events.get(1).unwrap().1["detail"],
        "marketplace tools is named kit"
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
