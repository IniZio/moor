//! Tests for `changes`.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, ListedRepo, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::request::ChangesReq;
use moor_core::seam::driver::{AcquireReq, Driver, Place};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxName, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::ops::common::changes::keep_diff;
use crate::testing::cx::{box_repo, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

/// A ready Box `t` on `o/n` and `o/docs`, and the Driver that holds it.
fn ready_with_two_repos(dir: &std::path::Path, driver: FakeDriver) -> (Cx, Arc<FakeDriver>) {
    let driver = Arc::new(driver);
    let cx = Cx::fake(dir, driver.clone(), Arc::new(ScriptedProc::new()));
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let mut repo = box_repo();
    repo.others.push(ListedRepo {
        repo: ForgeRepo::new("o", "docs"),
    });
    let new = Decision::insert(NewBox {
        name: name.clone(),
        repo: Some(repo),
        spec: SpecFields::default(),
    })
    .requester(REQUESTER_USER);
    let id = store.apply(new).unwrap().box_id;
    let provider = ProviderId::new("p");
    let fields = HandsFields {
        provider: Field::Set(provider.clone()),
        ..HandsFields::default()
    };
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(admitted).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle: got.handle });
    store.apply(ready).unwrap();
    (cx, driver)
}

#[test]
fn a_live_read_covers_every_repo_in_the_work_place_first_repo_first() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, driver) = ready_with_two_repos(dir.path(), FakeDriver::new());
    let req = ChangesReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
    };
    let view = run(&cx, &req).unwrap();
    assert!(view.live);
    let repos: Vec<_> = view.repos.iter().map(|r| r.repo.as_str()).collect();
    assert_eq!(repos, ["o/n", "o/docs"]);
    assert_eq!(driver.places("batch"), [Place::Work]);
}

/// What a snapshot, a numstat and a patch of one repo print.
fn repo_out<'a>(file: &'a str, patch: &'a str) -> [String; 3] {
    ["ok".to_owned(), format!("1\t0\t{file}\0"), patch.to_owned()]
}

/// Puts Box `t` to sleep as `sleep` does.
fn sleep(cx: &Cx, name: &BoxName) {
    let store = cx.store().unwrap();
    let row = store.open_box_by_name(name).unwrap().unwrap();
    store
        .apply(
            Decision::new(row.id)
                .lifecycle(BoxLifecycleInput::Sleep {
                    auto: false,
                    reason: None,
                })
                .hands_input(row.hands_inc(), HandsInput::Sleep),
        )
        .unwrap();
}

fn asked() -> ChangesReq {
    ChangesReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
    }
}

#[test]
fn an_asleep_box_shows_the_diff_kept_before_it_slept_without_waking() {
    let dir = tempfile::tempdir().unwrap();
    let main = repo_out("a.txt", "diff --git a/a.txt b/a.txt\n+x\n");
    let docs = repo_out("b.md", "diff --git a/b.md b/b.md\n+y\n");
    let texts: Vec<String> = main.into_iter().chain(docs).collect();
    let driver = FakeDriver::new().run_stdouts(texts.iter().map(String::as_str));
    let (cx, driver) = ready_with_two_repos(dir.path(), driver);
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let row = store.open_box_by_name(&name).unwrap().unwrap();
    keep_diff(&cx, store, &row);
    sleep(&cx, &name);
    let row = store.open_box_by_name(&name).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Asleep);
    let reads = driver.places("batch").len();
    let view = run(&cx, &asked()).unwrap();
    assert!(!view.live);
    assert_eq!(view.why.as_deref(), Some("the Box is asleep"));
    assert!(view.at.is_some());
    let got: Vec<_> = view
        .repos
        .iter()
        .map(|r| (r.repo.as_str(), r.patch.as_deref(), r.cut))
        .collect();
    assert_eq!(
        got,
        [
            ("o/n", Some("diff --git a/a.txt b/a.txt\n+x\n"), false),
            ("o/docs", Some("diff --git a/b.md b/b.md\n+y\n"), false),
        ]
    );
    assert_eq!(
        view.repos
            .first()
            .and_then(|r| r.files.first())
            .map(|f| f.path.as_str()),
        Some("a.txt")
    );
    assert_eq!(
        driver.places("batch").len(),
        reads,
        "a read of an asleep Box must not touch it"
    );
}

#[test]
fn the_same_diff_is_kept_once_and_a_changed_one_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let one = repo_out("a.txt", "diff one\n");
    let two = repo_out("a.txt", "diff two\n");
    let empty = || ["ok".to_owned(), String::new(), String::new()];
    let texts: Vec<String> = one
        .iter()
        .cloned()
        .chain(empty())
        .chain(one.clone())
        .chain(empty())
        .chain(two)
        .chain(empty())
        .collect();
    let driver = FakeDriver::new().run_stdouts(texts.iter().map(String::as_str));
    let (cx, _driver) = ready_with_two_repos(dir.path(), driver);
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let row = store.open_box_by_name(&name).unwrap().unwrap();
    keep_diff(&cx, store, &row);
    keep_diff(&cx, store, &row);
    assert_eq!(store.count_events_of(row.id, "diff_kept").unwrap(), 1);
    keep_diff(&cx, store, &row);
    assert_eq!(store.count_events_of(row.id, "diff_kept").unwrap(), 2);
    sleep(&cx, &name);
    let view = run(&cx, &asked()).unwrap();
    assert_eq!(
        view.repos.first().and_then(|r| r.patch.as_deref()),
        Some("diff two\n")
    );
}

#[test]
fn a_diff_past_the_budget_is_cut_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let line = format!("+{}\n", "x".repeat(99));
    let big = line.repeat(moor_core::changes::PATCH_BYTES / 100 + 50);
    let main = repo_out("a.txt", &big);
    let docs = repo_out("b.md", &format!("{}\n", "+".repeat(200)));
    let texts: Vec<String> = main.into_iter().chain(docs).collect();
    let driver = FakeDriver::new().run_stdouts(texts.iter().map(String::as_str));
    let (cx, _driver) = ready_with_two_repos(dir.path(), driver);
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let row = store.open_box_by_name(&name).unwrap().unwrap();
    keep_diff(&cx, store, &row);
    sleep(&cx, &name);
    let view = run(&cx, &asked()).unwrap();
    let kept: usize = view
        .repos
        .iter()
        .filter_map(|r| r.patch.as_ref())
        .map(String::len)
        .sum();
    assert!(kept <= moor_core::changes::PATCH_BYTES);
    let (Some(first), Some(second)) = (view.repos.first(), view.repos.get(1)) else {
        unreachable!("two repos");
    };
    assert!(first.cut);
    assert_eq!(
        second.patch.as_deref(),
        Some(""),
        "no budget is left for the next repo"
    );
    assert!(second.cut);
}

#[test]
fn an_asleep_box_with_no_kept_diff_still_answers_its_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _driver) = ready_with_two_repos(dir.path(), FakeDriver::new());
    sleep(&cx, &BoxName::parse_allow_bare("t").unwrap());
    let view = run(&cx, &asked()).unwrap();
    assert!(!view.live);
    assert!(view.repos.is_empty());
    assert!(view.at.is_none());
}
