//! `profile-check`: the forge read the start makes, with nothing written.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::sync::Arc;

use moor_core::domain::decision::Decision;
use moor_core::domain::event::ProfileStep;
use moor_core::name::{ForgeRepo, ProfileSource};
use moor_core::request::UserCmd;
use moor_core::seam::forge::ForgeError;
use moor_core::user::SecretName;
use moor_core::{BoxName, Sha};
use serde_json::{json, Value};

use crate::cx::Cx;
use crate::ops::common::profile_fetch::failed;
use crate::ops::users::run;
use crate::testing::cx::{new_box, requester, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::proc::ScriptedProc;
use crate::vault::Vault;

const HEAD: &str = "1111111111111111111111111111111111111111";
const URL: &str = "https://github.com/ann/profile";

const MANIFEST: &str = "claude:
  model: opus
  marketplaces:
    tools: https://github.com/ann/tools#v2
  plugins: [lint@tools]
codex:
  model: gpt
";

fn repo() -> ForgeRepo {
    ForgeRepo::new("ann", "profile")
}

fn sha() -> Sha {
    Sha::parse(HEAD).unwrap()
}

/// A context for the requester whose forge answers the profile repo's manifest with `manifest`
/// at its default branch, and `setup.sh` with `setup` at its head.
fn world(
    dir: &std::path::Path,
    manifest: Option<&str>,
    setup: Option<&str>,
) -> (Cx, Arc<FakeForge>) {
    let forge = Arc::new(
        FakeForge::new()
            .with_file(
                repo(),
                None,
                "moor-profile.yaml",
                Ok((sha(), manifest.map(str::to_owned))),
            )
            .with_file(
                repo(),
                Some(HEAD),
                "setup.sh",
                Ok((sha(), setup.map(str::to_owned))),
            ),
    );
    let cx = Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_forge(forge.clone());
    (cx, forge)
}

fn set(cx: &Cx, url: &str) {
    let source = ProfileSource::parse(url).unwrap();
    cx.store()
        .unwrap()
        .set_user_profile(REQUESTER_USER, &source)
        .unwrap();
}

fn check(cx: &Cx, source: Option<&str>) -> Value {
    let vault = Vault::load(cx.vault_key().unwrap()).unwrap();
    let cmd = UserCmd::ProfileCheck {
        source: source.map(|s| ProfileSource::parse(s).unwrap()),
    };
    run(cx, &vault, &requester(), &cmd).unwrap()
}

#[test]
fn a_valid_profile_reports_what_it_installs() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = world(dir.path(), Some(MANIFEST), Some("true\n"));
    set(&cx, URL);
    assert_eq!(
        check(&cx, None),
        json!({"source": URL, "failures": [], "last_failed": null, "resolved": {
            "sha": HEAD, "setup": true, "unresolved": [],
            "claude": {"model": "opus", "plugins": ["lint@tools"],
                "marketplaces": [{"name": "tools", "repo": "https://github.com/ann/tools#v2"}]},
            "codex": {"model": "gpt", "marketplaces": [], "plugins": []}}})
    );
}

#[test]
fn a_profile_without_setup_sh_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = world(dir.path(), None, None);
    set(&cx, URL);
    let got = check(&cx, None);
    assert_eq!(got["resolved"]["setup"], false);
    assert_eq!(got["failures"], json!([]));
}

#[test]
fn a_given_source_is_checked_instead_of_the_recorded_one_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, forge) = world(dir.path(), None, None);
    set(&cx, "https://github.com/ann/other");
    let got = check(&cx, Some(URL));
    assert_eq!(got["source"], URL);
    assert_eq!(got["resolved"]["sha"], HEAD);
    assert!(forge.files_asked().iter().all(|(r, _, _)| *r == repo()));
    let kept = cx.store().unwrap().user_profile(REQUESTER_USER).unwrap();
    assert_eq!(kept.unwrap().repo, ForgeRepo::new("ann", "other"));
}

#[test]
fn no_profile_reports_nothing_and_reads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, forge) = world(dir.path(), None, None);
    assert_eq!(
        check(&cx, None),
        json!({"source": null, "resolved": null, "failures": [], "last_failed": null})
    );
    assert!(forge.files_asked().is_empty());
}

#[test]
fn a_ref_that_does_not_resolve_is_a_resolve_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = world(dir.path(), None, None);
    let at = "https://github.com/ann/profile#nope";
    let forge = FakeForge::new().with_file(
        repo(),
        Some("nope"),
        "moor-profile.yaml",
        Err(ForgeError::NotVisible),
    );
    let cx = cx.with_forge(Arc::new(forge));
    let got = check(&cx, Some(at));
    assert_eq!(got["resolved"], Value::Null);
    assert_eq!(got["failures"][0]["step"], "resolve");
    assert!(got["failures"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("ann/profile"));
}

#[test]
fn a_bad_manifest_is_a_manifest_failure_with_the_commit_still_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = world(dir.path(), Some("nope: 1\n"), None);
    set(&cx, URL);
    let got = check(&cx, None);
    assert_eq!(got["resolved"]["sha"], HEAD);
    assert_eq!(got["resolved"]["claude"]["plugins"], json!([]));
    assert_eq!(got["failures"][0]["step"], "manifest");
}

#[test]
fn no_github_token_is_a_resolve_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, forge) = world(dir.path(), None, None);
    set(&cx, URL);
    let name = SecretName::parse("github").unwrap();
    cx.store()
        .unwrap()
        .delete_user_secret(REQUESTER_USER, &name)
        .unwrap();
    let got = check(&cx, None);
    assert_eq!(got["resolved"], Value::Null);
    assert_eq!(got["failures"][0]["step"], "resolve");
    assert!(got["failures"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("github"));
    assert!(forge.files_asked().is_empty());
}

#[test]
fn the_newest_failure_on_the_users_boxes_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = world(dir.path(), None, None);
    let store = cx.store().unwrap();
    let on = |name: &str| {
        let id = store
            .apply(new_box(BoxName::parse_allow_bare(name).unwrap()))
            .unwrap()
            .box_id;
        failed(
            &cx,
            store,
            id,
            ProfileStep::Fetch,
            format!("fetch on {name}"),
        )
        .unwrap();
    };
    assert_eq!(check(&cx, None)["last_failed"], Value::Null);
    on("first");
    on("second");
    let other = store
        .apply(Decision::insert(moor_core::domain::decision::NewBox {
            name: BoxName::parse_allow_bare("theirs").unwrap(),
            repo: None,
            spec: moor_core::domain::decision::SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    failed(
        &cx,
        store,
        other,
        ProfileStep::Setup,
        "not yours".to_owned(),
    )
    .unwrap();
    let last = check(&cx, None)["last_failed"].clone();
    assert_eq!(last["box_name"], "second");
    assert_eq!(last["step"], "fetch");
    assert_eq!(last["detail"], "fetch on second");
    assert!(last["at_ms"].as_i64().unwrap() > 0);
}
