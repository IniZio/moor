//! A start reads a repo's config from the store only: a stored row serves every Box of the repo,
//! a repo with none starts with an empty config, and neither says anything about it.

use moor_core::config::Flags;
use moor_core::seam::forge::{ForgeRepo, RepoAt};
use moor_core::{BoxId, ErrorKind, StartReq};

use super::{repo, repo_req, run, script_repo, world, World, BASE};
use crate::ops::common::facts::settings_of;
use crate::testing::cx::store_config;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

fn codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|w| w.code).collect()
}

fn start(w: &World, name: &str) -> BoxId {
    BoxId::new(run(&w.cx, &repo_req(name)).unwrap().boxed.id)
}

fn setup_of(w: &World, id: BoxId) -> Vec<String> {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    settings_of(&w.cx, &row, &Flags::default()).unwrap().setup
}

/// Whether any exec the driver ran names `needle`.
fn ran(w: &World, needle: &str) -> bool {
    (w.driver.execs().iter()).any(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(needle)))
}

#[test]
fn a_stored_config_serves_the_start_without_a_word() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("setup: [from-the-row]\nenv: {A: row}\n"));
    let id = start(&w, "demo");
    assert_eq!(codes(&w), Vec::<String>::new());
    assert_eq!(setup_of(&w, id), ["from-the-row"]);
    assert!(ran(&w, "from-the-row"), "{:?}", w.driver.execs());
}

#[test]
fn a_repo_with_no_stored_config_starts_with_an_empty_config_and_no_repo_config_warning() {
    let w = world(FakeDriver::new());
    script_repo(&w, None);
    let id = start(&w, "demo");
    let codes = codes(&w);
    assert!(
        !codes.iter().any(|c| c.starts_with("repo_config")),
        "{codes:?}"
    );
    assert_eq!(w.cx.store().unwrap().repo_config(&repo()).unwrap(), None);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let s = settings_of(&w.cx, &row, &Flags::default()).unwrap();
    assert!(s.setup.is_empty() && s.env.is_empty() && s.model.is_none());
}

#[test]
fn an_account_that_cannot_push_starts_on_the_stored_config() {
    let w = world(FakeDriver::new());
    store_config(w.cx.store().unwrap(), &repo(), "setup: [from-the-row]\n");
    let read_only = RepoAt {
        can_push: false,
        ..repo_at(BASE)
    };
    w.forge.answer(repo(), Ok(read_only));
    let id = start(&w, "demo");
    let codes = codes(&w);
    assert!(codes.contains(&"repo_read_only".to_owned()), "{codes:?}");
    assert!(
        !codes.iter().any(|c| c.starts_with("repo_config")),
        "{codes:?}"
    );
    assert_eq!(setup_of(&w, id), ["from-the-row"]);
}

#[test]
fn a_stored_config_that_does_not_parse_fails_the_start() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("bogus: 1\n"));
    let err = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("bogus"), "{}", err.error);
    assert!(w.driver.calls().is_empty(), "{:?}", w.driver.calls());
}

#[test]
fn a_repo_listed_beside_the_first_sets_up_on_its_own_stored_config() {
    let w = world(FakeDriver::new());
    let lib = ForgeRepo::new("o", "lib");
    script_repo(&w, None);
    w.forge.answer(lib.clone(), Ok(repo_at(BASE)));
    store_config(w.cx.store().unwrap(), &lib, "setup: [lib-row]\n");
    let req = StartReq {
        also: Some(vec![lib.clone()]),
        ..repo_req("demo")
    };
    run(&w.cx, &req).unwrap();
    let codes = codes(&w);
    assert!(
        !codes.iter().any(|c| c.starts_with("repo_config")),
        "{codes:?}"
    );
    assert!(ran(&w, "lib-row"), "{:?}", w.driver.execs());

    store_config(w.cx.store().unwrap(), &lib, "setup: [lib-edited]\n");
    let again = StartReq {
        also: Some(vec![lib]),
        ..repo_req("again")
    };
    run(&w.cx, &again).unwrap();
    assert!(ran(&w, "lib-edited"), "{:?}", w.driver.execs());
}
