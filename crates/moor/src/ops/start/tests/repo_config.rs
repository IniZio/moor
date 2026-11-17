//! A start imports a repo's `.moor.yaml` into its stored config when none exists and the account
//! may push, and ignores the file, with a warning, once a stored config disagrees with it.

use moor_core::config::Flags;
use moor_core::domain::decision::ListedRepo;
use moor_core::seam::forge::{ForgeRepo, RepoAt};
use moor_core::{BoxId, ErrorKind, StartReq};

use super::{repo, repo_req, run, script_base_yaml, world, World, BASE};
use crate::ops::common::facts::settings_of;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

const FILE: &str = "setup: [from-the-file]\nenv: {A: file}\n";

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

#[test]
fn a_pushing_account_imports_the_file_and_is_told() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(FILE));
    let id = start(&w, "demo");
    let warned = w.cx.take_warnings();
    let imported: Vec<_> = warned
        .iter()
        .filter(|w| w.code == "repo_config_imported")
        .collect();
    assert_eq!(imported.len(), 1, "{warned:?}");
    assert!(
        imported.first().unwrap().message.contains("o/n"),
        "{}",
        imported.first().unwrap().message
    );
    let row = w.cx.store().unwrap().repo_config(&repo()).unwrap().unwrap();
    assert_eq!(
        (row.text.as_str(), row.revision, row.edited_by.as_str()),
        (FILE, 1, "import")
    );
    assert_eq!(setup_of(&w, id), ["from-the-file"]);
}

#[test]
fn a_second_start_with_the_same_file_says_nothing_more() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some(FILE));
    start(&w, "one");
    w.cx.take_warnings();
    start(&w, "two");
    let codes = codes(&w);
    assert!(
        !codes.iter().any(|c| c.starts_with("repo_config")),
        "{codes:?}"
    );
    let row = w.cx.store().unwrap().repo_config(&repo()).unwrap().unwrap();
    assert_eq!(row.revision, 1);
}

#[test]
fn a_file_that_disagrees_with_the_stored_config_is_ignored_with_a_warning() {
    let w = world(FakeDriver::new());
    w.cx.store()
        .unwrap()
        .set_repo_config(&repo(), "setup: [from-the-row]\n", "operator", Some(0))
        .unwrap();
    script_base_yaml(&w, Some(FILE));
    let id = start(&w, "demo");
    let warned = w.cx.take_warnings();
    let ignored: Vec<_> = warned
        .iter()
        .filter(|w| w.code == "repo_config_file_ignored")
        .collect();
    assert_eq!(ignored.len(), 1, "{warned:?}");
    assert!(
        ignored.first().unwrap().message.contains("setup, env"),
        "{}",
        ignored.first().unwrap().message
    );
    assert!(!warned.iter().any(|w| w.code == "repo_config_imported"));
    assert_eq!(setup_of(&w, id), ["from-the-row"]);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let stored = row.repo.unwrap().moor_yaml;
    assert_eq!(stored.as_deref(), Some("setup: [from-the-row]\n"));
}

#[test]
fn a_stored_config_serves_a_repo_whose_file_is_gone() {
    let w = world(FakeDriver::new());
    w.cx.store()
        .unwrap()
        .set_repo_config(&repo(), "setup: [from-the-row]\n", "operator", Some(0))
        .unwrap();
    script_base_yaml(&w, None);
    let id = start(&w, "demo");
    assert_eq!(codes(&w), Vec::<String>::new());
    assert_eq!(setup_of(&w, id), ["from-the-row"]);
}

#[test]
fn an_account_that_cannot_push_uses_the_file_for_its_own_box_and_imports_nothing() {
    let w = world(FakeDriver::new());
    let read_only = RepoAt {
        can_push: false,
        ..repo_at(BASE, Some(FILE))
    };
    w.forge.answer(repo(), Ok(read_only));
    let id = start(&w, "demo");
    assert!(!codes(&w).contains(&"repo_config_imported".to_owned()));
    assert_eq!(w.cx.store().unwrap().repo_config(&repo()).unwrap(), None);
    assert_eq!(setup_of(&w, id), ["from-the-file"]);
}

#[test]
fn a_file_that_does_not_parse_fails_the_start_and_imports_nothing() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some("bogus: 1\n"));
    let err = run(&w.cx, &repo_req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("bogus"), "{}", err.error);
    assert_eq!(w.cx.store().unwrap().repo_config(&repo()).unwrap(), None);
}

#[test]
fn a_repo_listed_beside_the_first_looks_up_and_imports_its_own_config() {
    let w = world(FakeDriver::new());
    let lib = ForgeRepo::new("o", "lib");
    script_base_yaml(&w, None);
    w.forge
        .answer(lib.clone(), Ok(repo_at(BASE, Some("setup: [lib-file]\n"))));
    let req = StartReq {
        also: Some(vec![lib.clone()]),
        ..repo_req("demo")
    };
    let id = BoxId::new(run(&w.cx, &req).unwrap().boxed.id);
    assert!(codes(&w).contains(&"repo_config_imported".to_owned()));
    let row = w.cx.store().unwrap().repo_config(&lib).unwrap().unwrap();
    assert_eq!(row.text, "setup: [lib-file]\n");

    w.cx.store()
        .unwrap()
        .set_repo_config(&lib, "setup: [lib-row]\n", "operator", Some(1))
        .unwrap();
    let second = StartReq {
        also: Some(vec![lib.clone()]),
        ..repo_req("again")
    };
    let again = BoxId::new(run(&w.cx, &second).unwrap().boxed.id);
    let listed = |id| {
        let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
        row.repo.unwrap().others
    };
    assert_eq!(
        listed(id),
        vec![ListedRepo {
            repo: lib.clone(),
            moor_yaml: Some("setup: [lib-file]\n".to_owned())
        }]
    );
    assert_eq!(
        listed(again),
        vec![ListedRepo {
            repo: lib,
            moor_yaml: Some("setup: [lib-row]\n".to_owned())
        }]
    );
}
