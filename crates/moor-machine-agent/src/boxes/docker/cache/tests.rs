#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use moor_client::link::lease::{Cache, CacheVolume};

use super::{args, mounts};
use crate::boxes::Slug;

const SLUG: &str = "moor-3b3b3b3b-9001";

fn vol(group: &str, name: &str, paths: &[&str]) -> CacheVolume {
    CacheVolume {
        group: group.to_owned(),
        name: name.to_owned(),
        key: format!("{group}-{name}"),
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        size_gb: 0,
    }
}

fn cache(volumes: Vec<CacheVolume>) -> Cache {
    Cache {
        volumes,
        scope: None,
    }
}

fn slug() -> Slug {
    Slug::parse(SLUG).unwrap()
}

const G: &str = "0123456789abcdef";

#[test]
fn no_cache_means_no_mounts() {
    assert!(mounts(&slug(), None).unwrap().is_empty());
    assert!(args(&[], true).is_empty());
}

#[test]
fn a_package_cache_mounts_the_shared_volume_itself() {
    let c = cache(vec![vol(G, "rust", &["~/.cargo/registry", "~/.cargo/git"])]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].volume(), format!("moor-cache-{G}-rust-0"));
    assert_eq!(m[1].volume(), format!("moor-cache-{G}-rust-1"));
    assert_eq!(m[0].path, "/home/agent/.cargo/registry");
    assert!(m.iter().all(|m| m.private.is_none()));
}

#[test]
fn what_two_live_boxes_must_not_share_is_copied() {
    let tree = format!("{G}w");
    let c = cache(vec![
        vol(&tree, "worktree", &["~/moor/repos/r"]),
        vol(G, "docker", &["~/docker", "/var/lib/containerd"]),
        vol(&format!("{G}x"), "cache", &["~/.cache"]),
        vol(&format!("{G}a"), "agent", &["~/.claude"]),
        vol(G, "mine", &["~/data"]),
    ]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert_eq!(m.len(), 6);
    assert!(m.iter().all(|m| m.private.is_some()), "{m:?}");
    assert!(m[0].private.as_deref().unwrap().starts_with(SLUG));
    assert_ne!(m[0].volume(), m[0].canonical);
}

#[test]
fn a_hex_group_ending_in_a_is_still_a_shared_package_cache() {
    let c = cache(vec![vol("0123456789abcdea", "go", &["~/go/pkg/mod"])]);
    assert!(mounts(&slug(), Some(&c)).unwrap()[0].private.is_none());
}

#[test]
fn the_work_tree_reaches_both_places_and_other_caches_only_the_work_place() {
    let c = cache(vec![
        vol(&format!("{G}w"), "worktree", &["~/moor/repos/r"]),
        vol(G, "go", &["~/go/pkg/mod"]),
    ]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    let agent = args(&m, false);
    assert_eq!(agent.len(), 2);
    assert!(agent[1].ends_with(":/home/agent/moor/repos/r"), "{agent:?}");
    let work = args(&m, true);
    assert_eq!(work.len(), 6);
    assert_eq!(
        work.last().unwrap(),
        "MOOR_CACHE_DIRS=/home/agent/moor/repos/r:/home/agent/go/pkg/mod"
    );
}

#[test]
fn a_path_or_name_docker_could_misread_is_refused() {
    for bad in ["~/a:b", "~/a,b", "/opt/../etc", "~/a b", "relative"] {
        let c = cache(vec![vol(G, "x", &[bad])]);
        assert!(mounts(&slug(), Some(&c)).is_err(), "{bad}");
    }
    let c = cache(vec![vol(G, "X", &["~/.cache"])]);
    assert!(mounts(&slug(), Some(&c)).is_err());
}

#[test]
fn two_volumes_may_not_claim_one_path() {
    let c = cache(vec![
        vol(G, "a", &["~/.cache"]),
        vol("fedcba9876543210", "b", &["~/.cache"]),
    ]);
    assert!(mounts(&slug(), Some(&c)).is_err());
}

#[test]
fn the_volume_names_hold_the_key_and_so_never_collide_across_repos() {
    let a = mounts(&slug(), Some(&cache(vec![vol(G, "go", &["~/go/pkg/mod"])]))).unwrap();
    let b = mounts(
        &slug(),
        Some(&cache(vec![vol(
            "fedcba9876543210",
            "go",
            &["~/go/pkg/mod"],
        )])),
    )
    .unwrap();
    assert_ne!(a[0].canonical, b[0].canonical);
}
