//! How a start finds the commit of each repo a profile installs from.
#![allow(clippy::indexing_slicing, reason = "tests")]

use std::time::Duration;

use moor_core::domain::event::ResolvedSource;
use moor_core::seam::engine::GitSource;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{Secret, Sha};

use super::*;
use crate::ops::start::profile::{resolve_sources, SOURCE_LOOKUP};
use crate::testing::forge::FakeForge;

const COMMIT: &str = "1111111111111111111111111111111111111111";

const OTHER: &str = "2222222222222222222222222222222222222222";

fn tools() -> ForgeRepo {
    ForgeRepo::new("o", "tools")
}

const URL: &str = "https://github.com/o/tools";

/// A forge where `o/tools` answers [`COMMIT`] on its default branch and [`OTHER`] at `v2`.
fn forge() -> FakeForge {
    let at = |sha: &str| Ok((Sha::parse(sha).unwrap(), None));
    FakeForge::new()
        .with_repo(URL, tools())
        .with_file(tools(), None, SOURCE_LOOKUP, at(COMMIT))
        .with_file(tools(), Some("v2"), SOURCE_LOOKUP, at(OTHER))
}

fn source(url: &str, git_ref: Option<&str>) -> GitSource {
    GitSource {
        url: url.to_owned(),
        git_ref: git_ref.map(str::to_owned),
    }
}

fn resolved(url: &str, sha: &str) -> ResolvedSource {
    ResolvedSource {
        url: url.to_owned(),
        sha: sha.to_owned(),
    }
}

/// What a start with `forge` makes of `listed`, within `bound`, with the commits sorted.
fn resolve(
    forge: FakeForge,
    listed: Vec<GitSource>,
    bound: Duration,
) -> (Vec<ResolvedSource>, Vec<String>) {
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_forge(Arc::new(forge));
    let token = Secret::new(b"t".to_vec());
    let (mut found, why) = resolve_sources(&w.cx, listed, token, bound);
    found.sort_by(|a, b| a.sha.cmp(&b.sha));
    (found, why)
}

const PATIENT: Duration = Duration::from_mins(1);

#[test]
fn one_repo_listed_at_two_refs_has_both_commits_in_the_answer() {
    let listed = vec![source(URL, None), source(URL, Some("v2"))];
    let (found, why) = resolve(forge(), listed, PATIENT);
    assert_eq!(found, [resolved(URL, COMMIT), resolved(URL, OTHER)]);
    assert_eq!(why, [""; 0]);
}

#[test]
fn a_repo_that_does_not_answer_in_time_is_unresolved_and_says_so() {
    let forge = forge().with_slow_files(tools(), Duration::from_secs(1));
    let listed = vec![source(URL, None)];
    let (found, why) = resolve(forge, listed, Duration::from_millis(20));
    assert_eq!(found, []);
    assert_eq!(why.len(), 1);
    assert!(
        why[0].contains(URL) && why[0].contains("no answer"),
        "{why:?}"
    );
}

#[test]
fn a_source_that_is_not_on_the_forge_is_unresolved() {
    let other = "https://example.com/o/tools.git";
    let (found, why) = resolve(forge(), vec![source(other, None)], PATIENT);
    assert_eq!(found, []);
    assert!(why[0].contains(other), "{why:?}");
}

#[test]
fn only_a_whole_commit_id_skips_the_lookup() {
    let listed = vec![
        source(URL, Some("v2")),
        source("not-a-forge-url", Some(COMMIT)),
    ];
    let (found, why) = resolve(forge(), listed, PATIENT);
    assert_eq!(
        found,
        [resolved("not-a-forge-url", COMMIT), resolved(URL, OTHER)]
    );
    assert_eq!(why, [""; 0]);
}
