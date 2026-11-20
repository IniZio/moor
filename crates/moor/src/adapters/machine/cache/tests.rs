#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use moor_core::cache::{CacheLayers, CachePlan, CacheSalt};
use moor_core::config::parse_repo;
use moor_core::name::ForgeRepo;
use moor_core::user::UserId;

use super::wire;

fn plan(repo_yaml: &str) -> CachePlan {
    let layers = CacheLayers {
        repo: parse_repo("repo config", repo_yaml).unwrap().cache,
        ..CacheLayers::default()
    };
    let repo = ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    CachePlan::build(
        UserId::new(1),
        Some(&repo),
        &layers,
        &CacheSalt::new([3; 64]),
    )
    .unwrap()
}

#[test]
fn a_cold_plan_sends_no_cache() {
    assert_eq!(wire(&CachePlan::none()), None);
}

#[test]
fn a_plan_goes_to_the_agent_with_its_keys_paths_and_scope() {
    let p = plan("cache: {presets: [pnpm], work_tree: false, volumes: [{name: sdk, path: /opt/sdk, size_gb: 3}]}");
    let sent = wire(&p).unwrap();
    assert_eq!(sent.scope.as_deref(), p.scope());
    assert!(sent.scope.is_some());
    let cache = sent.volumes.iter().find(|v| v.name == "cache").unwrap();
    assert_eq!(cache.paths, ["~/.cache"]);
    assert_eq!(cache.key, format!("{}-cache", cache.group));
    let sdk = sent.volumes.iter().find(|v| v.name == "sdk").unwrap();
    assert_eq!(
        (sdk.paths.clone(), sdk.size_gb),
        (vec!["/opt/sdk".to_owned()], 3)
    );
}

#[test]
fn another_repo_gets_other_keys() {
    let one = wire(&plan("cache: {work_tree: false}")).unwrap();
    let layers = CacheLayers::default();
    let repo = ForgeRepo {
        owner: "o".to_owned(),
        name: "other".to_owned(),
    };
    let other = wire(
        &CachePlan::build(
            UserId::new(1),
            Some(&repo),
            &layers,
            &CacheSalt::new([3; 64]),
        )
        .unwrap(),
    )
    .unwrap();
    assert_ne!(one.volumes[0].key, other.volumes[0].key);
}
