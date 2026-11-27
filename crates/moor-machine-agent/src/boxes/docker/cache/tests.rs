#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use moor_client::link::lease::{Cache, CacheVolume};

use std::time::Duration;

use super::expire::{line, parse_age, verdict, Verdict, DEFAULT_MAX_AGE};
use super::marker::{marker, standing, Private};
use super::uses::Uses;
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
    let c = cache(vec![
        vol(G, "docker", &["~/docker", "/var/lib/containerd"]),
        vol(&format!("{G}x"), "cache", &["~/.cache"]),
        vol(&format!("{G}a"), "agent", &["~/.claude"]),
        vol(G, "mine", &["~/data"]),
    ]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert_eq!(m.len(), 5);
    assert!(m.iter().all(|m| m.private.is_some()), "{m:?}");
    assert!(m[0].private.as_deref().unwrap().starts_with(SLUG));
    assert_ne!(m[0].volume(), m[0].canonical);
}

#[test]
fn the_git_mirrors_are_mounted_shared_by_the_repo_s_boxes() {
    let c = cache(vec![vol(G, "git", &["~/.local/share/git-mirror"])]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert_eq!(m[0].volume(), format!("moor-cache-{G}-git-0"));
    assert_eq!(m[0].path, "/home/agent/.local/share/git-mirror");
    assert!(m[0].private.is_none());
}

#[test]
fn a_hex_group_ending_in_a_is_still_a_shared_package_cache() {
    let c = cache(vec![vol("0123456789abcdea", "go", &["~/go/pkg/mod"])]);
    assert!(mounts(&slug(), Some(&c)).unwrap()[0].private.is_none());
}

#[test]
fn a_package_cache_reaches_only_the_work_place() {
    let c = cache(vec![vol(G, "go", &["~/go/pkg/mod"])]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert!(args(&m, false).is_empty());
    let work = args(&m, true);
    assert_eq!(work.len(), 4);
    assert_eq!(
        work.last().unwrap(),
        "MOOR_CACHE_DIRS=/home/agent/go/pkg/mod"
    );
}

#[test]
fn what_the_profile_installs_reaches_both_places_as_a_private_copy() {
    let c = cache(vec![
        vol(
            G,
            "tools",
            &["~/.bun/bin", "~/.local/bin", "~/.claude/plugins"],
        ),
        vol(G, "go", &["~/go/pkg/mod"]),
    ]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    assert!(m[..3].iter().all(|m| m.private.is_some()), "{m:?}");
    let agent = args(&m, false);
    let mounted: Vec<&str> = agent
        .iter()
        .skip(1)
        .step_by(2)
        .map(String::as_str)
        .collect();
    let tools = |n: usize, path: &str| format!("{SLUG}-cache-tools-{n}:/home/agent/{path}");
    assert_eq!(
        mounted,
        [
            tools(0, ".bun/bin"),
            tools(1, ".local/bin"),
            tools(2, ".claude/plugins")
        ]
    );
    assert_eq!(args(&m, true).len(), 10);
}

#[test]
fn the_gh_extensions_a_profile_installs_reach_both_places_too() {
    let c = cache(vec![vol(
        G,
        "tools",
        &["~/.local/bin", "~/.local/share/gh/extensions"],
    )]);
    let m = mounts(&slug(), Some(&c)).unwrap();
    let agent = args(&m, false);
    assert!(
        agent
            .iter()
            .any(|a| a.ends_with(":/home/agent/.local/share/gh/extensions")),
        "{agent:?}"
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

#[test]
fn a_private_copy_without_its_marker_is_made_again() {
    assert_eq!(standing(false, false), Private::Make);
    assert_eq!(standing(true, false), Private::Redo);
    assert_eq!(standing(true, true), Private::Trust);
}

#[test]
fn the_marker_is_its_own_volume_named_from_the_private_one() {
    let m = mounts(&slug(), Some(&cache(vec![vol(G, "mine", &["~/data"])]))).unwrap();
    let private = m[0].private.as_deref().unwrap();
    assert_eq!(marker(private), format!("{private}.ok"));
    assert_ne!(marker(private), private);
}

const DAY: Duration = Duration::from_hours(24);

#[test]
fn the_age_is_thirty_days_unless_set_and_zero_turns_expiry_off() {
    assert_eq!(DEFAULT_MAX_AGE, 30 * DAY);
    assert_eq!(parse_age("30d").unwrap(), 30 * DAY);
    assert_eq!(parse_age("7").unwrap(), 7 * DAY);
    assert_eq!(parse_age("12h").unwrap(), Duration::from_hours(12));
    assert_eq!(parse_age("20s").unwrap(), Duration::from_secs(20));
    assert_eq!(parse_age("0").unwrap(), Duration::ZERO);
    assert!(parse_age("soon").is_err());
    assert!(parse_age("5w").is_err());
}

#[test]
fn nothing_goes_when_expiry_is_off() {
    assert_eq!(
        verdict(Some(0), 10 * 365 * 86_400, Duration::ZERO, false),
        Verdict::Off
    );
}

#[test]
fn a_volume_with_no_recorded_use_starts_its_clock_and_stays() {
    assert_eq!(verdict(None, 1_000_000, DAY, false), Verdict::Learn);
}

#[test]
fn a_volume_used_within_the_age_stays() {
    assert_eq!(
        verdict(Some(1_000), 1_000 + 86_399, DAY, false),
        Verdict::Fresh
    );
}

#[test]
fn an_old_volume_no_box_holds_goes() {
    assert_eq!(
        verdict(Some(1_000), 1_000 + 86_400, DAY, false),
        Verdict::Expire
    );
}

#[test]
fn an_old_volume_a_live_or_asleep_box_holds_stays() {
    assert_eq!(verdict(Some(1_000), 10_000_000, DAY, true), Verdict::Held);
}

#[test]
fn the_log_line_names_the_volume_and_its_size() {
    let said = line(
        "moor-cache-0123456789abcdef-go-0",
        Some(4_914_000_000),
        90_000,
    );
    assert!(said.contains("moor-cache-0123456789abcdef-go-0"), "{said}");
    assert!(said.contains("4914000000 bytes"), "{said}");
    assert!(line("v", None, 1).contains("size unknown"));
}

#[test]
fn use_is_recorded_kept_across_restarts_and_forgotten_with_the_volume() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cache-use.json");
    let uses = Uses::default();
    uses.keep_in(file.clone());
    assert_eq!(uses.last("a"), None);
    uses.touch_at(["a", "b"], 100);
    uses.touch_at(["a"], 200);
    assert_eq!((uses.last("a"), uses.last("b")), (Some(200), Some(100)));
    let again = Uses::default();
    again.keep_in(file.clone());
    assert_eq!((again.last("a"), again.last("b")), (Some(200), Some(100)));
    again.forget("a");
    let third = Uses::default();
    third.keep_in(file);
    assert_eq!((third.last("a"), third.last("b")), (None, Some(100)));
}

#[test]
fn a_new_open_counts_as_use_now() {
    let uses = Uses::default();
    uses.touch(["moor-cache-x-go-0"]);
    let at = uses.last("moor-cache-x-go-0").unwrap();
    assert!((crate::unix_now() - at).abs() < 5);
}
