//! The profile's `env` reaches the agent launch, minus the names it may not set and the names the
//! repo's `env` sets.
#![allow(
    clippy::indexing_slicing,
    clippy::literal_string_with_formatting_args,
    reason = "tests"
)]

use moor_core::Warning;

use super::profile::{set_profile, world_answering, HEAD};
use super::*;
use crate::testing::engine::FakeEngine;

/// Name and value pairs of an `env`.
type Env = Vec<(String, String)>;

const PROFILE: &str =
    "claude:\n  env:\n    BUN_OPTIONS: \"--preload={profile}/a.js\"\n    NOTE: mine\n";

/// Starts `demo` of [`repo`] for a requester whose profile is `manifest` and whose `.moor.yaml`
/// is `yaml`; returns the world, the launch's profile `env` and its repo `env`.
fn launch(manifest: &str, yaml: &str) -> (World, Env, Env) {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world_answering(None, HEAD, Some(manifest));
    set_profile(&w, None);
    w.cx = w.cx.with_engine(engine.clone());
    script_base_yaml(&w, Some(yaml));
    run(&w.cx, &repo_req("demo")).unwrap();
    let launches = engine.launches();
    let sent = launches.first().expect("the Box still starts");
    let profile = sent.profile.as_ref().unwrap().claude.env.clone();
    let repo = sent.proxy.as_ref().unwrap().box_env.clone();
    (w, profile.into_iter().collect(), repo)
}

fn profile_warnings(w: &World) -> Vec<Warning> {
    let all = w.cx.take_warnings();
    all.into_iter()
        .filter(|x| x.code == "profile_failed")
        .collect()
}

#[test]
fn the_profile_env_reaches_the_launch_untouched() {
    let (w, profile, _) = launch(PROFILE, "setup: [a]\n");
    let names: Vec<&str> = profile.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["BUN_OPTIONS", "NOTE"]);
    assert_eq!(profile[0].1, "--preload={profile}/a.js");
    assert!(profile_warnings(&w).is_empty());
}

#[test]
fn a_refused_name_is_dropped_with_a_profile_warning_and_the_box_starts() {
    let manifest =
        "claude:\n  env: {PATH: x, DEPLOY_TOKEN: y, NODE_OPTIONS: z, \"BAD-NAME\": w, KEEP: k}\n";
    let (w, profile, _) = launch(manifest, "setup: [a]\n");
    assert_eq!(profile, [("KEEP".to_owned(), "k".to_owned())]);
    let warned = profile_warnings(&w);
    assert_eq!(warned.len(), 4, "{warned:?}");
    for name in ["PATH", "DEPLOY_TOKEN", "NODE_OPTIONS", "BAD-NAME"] {
        let n = warned.iter().filter(|x| x.message.contains(name)).count();
        assert_eq!(n, 1, "{name}: {warned:?}");
    }
}

#[test]
fn the_repo_env_wins_over_the_profile_env_and_the_warning_names_the_variable() {
    let (w, profile, repo) = launch(PROFILE, "env:\n  BUN_OPTIONS: --repo-flag\n");
    assert_eq!(profile, [("NOTE".to_owned(), "mine".to_owned())]);
    let value = repo.iter().find(|(n, _)| n == "BUN_OPTIONS");
    assert_eq!(value.map(|(_, v)| v.as_str()), Some("--repo-flag"));
    let warned = profile_warnings(&w);
    assert_eq!(warned.len(), 1, "{warned:?}");
    let message = &warned[0].message;
    assert!(
        message.contains("BUN_OPTIONS") && message.contains("repo"),
        "{message}"
    );
}
