//! `pulls`: each repo's pull request from the Box's branch, first repo first, read once a while.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, ListedRepo, NewBox, SpecFields};
use moor_core::request::BoxReq;
use moor_core::seam::forge::{BranchPull, Checks, ForgeError, ForgeRepo, Pull, PullState};
use moor_core::BoxName;

use super::read_through;
use crate::cx::pulls::{PullCache, FRESH};
use crate::cx::Cx;
use crate::testing::cx::{box_repo, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::proc::ScriptedProc;

const BRANCH: &str = "moor/7-fix-typo";

fn other() -> ForgeRepo {
    ForgeRepo::new("o", "docs")
}

/// A pushed branch with `pull`.
const fn pushed(pull: Option<Pull>) -> BranchPull {
    BranchPull { pushed: true, pull }
}

fn req() -> BoxReq {
    BoxReq {
        box_name: BoxName::parse_allow_bare("fix-typo").unwrap(),
    }
}

/// A context whose store holds Box `fix-typo` on `o/n` and `o/docs`, on [`BRANCH`].
fn with_box(dir: &std::path::Path, forge: &Arc<FakeForge>) -> Cx {
    let cx = Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_forge(forge.clone());
    let mut repo = box_repo();
    repo.others.push(ListedRepo { repo: other() });
    let new = Decision::insert(NewBox {
        name: req().box_name,
        repo: Some(repo),
        spec: SpecFields {
            branch: Some(BRANCH.to_owned()),
            base: Some("66166358f418d2007cbeb44ec401062a1cb72094".to_owned()),
            base_ref: Some("main".to_owned()),
            ..SpecFields::default()
        },
    })
    .requester(REQUESTER_USER);
    cx.store().unwrap().apply(new).unwrap();
    cx
}

#[test]
fn each_repo_answers_its_pull_from_the_box_branch_first_repo_first() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(FakeForge::new());
    let draft = Pull {
        number: 12,
        state: PullState::Draft,
        checks: Some(Checks::Pending),
    };
    forge.answer_pull(box_repo().repo, BRANCH, Ok(pushed(Some(draft.clone()))));
    forge.answer_pull(other(), BRANCH, Err(ForgeError::NotVisible));
    let cx = with_box(dir.path(), &forge);
    let got = read_through(&cx, &req(), &PullCache::default()).unwrap();
    assert_eq!(got.branch.as_deref(), Some(BRANCH));
    assert_eq!(got.base.as_deref(), Some("main"));
    assert!(got.repos[0].pushed);
    assert!(!got.repos[1].pushed);
    let repos: Vec<_> = got.repos.iter().map(|r| r.repo.clone()).collect();
    assert_eq!(repos, [box_repo().repo, other()]);
    assert_eq!(got.repos[0].pull, Some(draft));
    assert_eq!(got.repos[1].pull, None);
    let why = got.repos[1].unread.as_deref().unwrap_or_default();
    assert!(why.contains("not visible"), "{why}");
}

#[test]
fn a_poll_within_the_fresh_window_does_not_ask_the_forge_again() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(FakeForge::new());
    let cx = with_box(dir.path(), &forge);
    let cache = PullCache::default();
    let first = read_through(&cx, &req(), &cache).unwrap();
    assert_eq!(forge.pulls_asked(), 2);
    let open = Pull {
        number: 3,
        state: PullState::Open,
        checks: Some(Checks::Passing),
    };
    forge.answer_pull(box_repo().repo, BRANCH, Ok(pushed(Some(open))));
    assert_eq!(read_through(&cx, &req(), &cache).unwrap(), first);
    assert_eq!(forge.pulls_asked(), 2);
}

#[test]
fn a_held_read_lasts_only_the_fresh_window() {
    let cache = PullCache::default();
    let key = (moor_core::BoxId::new(1), box_repo().repo);
    let at = moor_core::seam::clock::Ts::from_millis(1_000);
    cache.put(key.clone(), at, Ok(pushed(None)));
    assert_eq!(
        cache.get(&key, at.saturating_add(Duration::from_secs(29))),
        Some(Ok(pushed(None)))
    );
    assert_eq!(cache.get(&key, at.saturating_add(FRESH)), None);
    let other_box = (moor_core::BoxId::new(2), box_repo().repo);
    assert_eq!(cache.get(&other_box, at), None);
}
