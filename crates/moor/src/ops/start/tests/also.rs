//! A start may list repos the Box clones beside its primary one.

use moor_core::config::UserConfig;
use moor_core::domain::decision::ListedRepo;
use moor_core::seam::forge::{ForgeRepo, RepoAt};
use moor_core::{BoxId, ErrorKind, StartReq};

use super::{repo, repo_req, run, script_base_yaml, world, world_with, World, BASE};
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

fn lib() -> ForgeRepo {
    ForgeRepo::new("o", "lib")
}

fn deploy() -> ForgeRepo {
    ForgeRepo::new("p", "deploy")
}

/// Scripts the primary and both other repos, each with its own `.moor.yaml`.
fn scripted(w: &World) {
    script_base_yaml(w, Some("setup: [main]\n"));
    w.forge
        .answer(lib(), Ok(repo_at(BASE, Some("setup: [lib]\n"))));
    w.forge.answer(deploy(), Ok(repo_at(BASE, None)));
}

fn listing(name: &str, also: Vec<ForgeRepo>) -> StartReq {
    StartReq {
        also: Some(also),
        ..repo_req(name)
    }
}

fn others(w: &World, id: BoxId) -> Vec<ListedRepo> {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    row.repo.expect("repo").others
}

fn started(w: &World, req: &StartReq) -> BoxId {
    BoxId::new(run(&w.cx, req).unwrap().boxed.id)
}

fn listed(repo: ForgeRepo, moor_yaml: Option<&str>) -> ListedRepo {
    ListedRepo {
        repo,
        moor_yaml: moor_yaml.map(str::to_owned),
    }
}

#[test]
fn a_two_repo_start_records_the_others_in_order_with_their_configs() {
    let w = world(FakeDriver::new());
    scripted(&w);
    let id = started(&w, &listing("demo", vec![deploy(), lib()]));
    assert_eq!(
        others(&w, id),
        vec![
            listed(deploy(), None),
            listed(lib(), Some("setup: [lib]\n"))
        ]
    );
}

#[test]
fn a_listed_repo_the_account_cannot_push_to_is_refused_by_name() {
    let w = world(FakeDriver::new());
    scripted(&w);
    let read_only = RepoAt {
        can_push: false,
        ..repo_at(BASE, None)
    };
    w.forge.answer(deploy(), Ok(read_only));
    let err = run(&w.cx, &listing("demo", vec![lib(), deploy()])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("p/deploy"), "{}", err.error);
    assert!(w.driver.calls().is_empty());
}

#[test]
fn a_repo_listed_twice_or_two_repos_of_one_name_are_refused() {
    for (also, named) in [
        (vec![lib(), lib()], "o/lib"),
        (vec![repo()], "o/n"),
        (vec![lib(), ForgeRepo::new("q", "lib")], "lib"),
    ] {
        let w = world(FakeDriver::new());
        scripted(&w);
        let err = run(&w.cx, &listing("demo", also)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{named}");
        assert!(err.error.to_string().contains(named), "{}", err.error);
        assert!(w.driver.calls().is_empty(), "{named}");
    }
}

fn configured() -> World {
    let mut config = UserConfig::default();
    config.box_.also = vec![lib()];
    let w = world_with(FakeDriver::new(), config);
    scripted(&w);
    w
}

#[test]
fn the_configured_list_applies_when_the_start_names_none() {
    let w = configured();
    let id = started(&w, &repo_req("demo"));
    assert_eq!(others(&w, id), vec![listed(lib(), Some("setup: [lib]\n"))]);
}

#[test]
fn a_listed_set_replaces_the_configured_one() {
    let w = configured();
    let id = started(&w, &listing("demo", vec![deploy()]));
    assert_eq!(others(&w, id), vec![listed(deploy(), None)]);
    let id = started(&w, &listing("solo", Vec::new()));
    assert!(others(&w, id).is_empty());
}

#[test]
fn a_one_repo_start_reads_only_its_repo() {
    let w = world(FakeDriver::new());
    scripted(&w);
    let id = started(&w, &repo_req("demo"));
    assert!(others(&w, id).is_empty());
    assert_eq!(w.forge.reads().len(), 1);
}

#[test]
fn starting_an_open_box_with_another_list_is_refused() {
    let w = world(FakeDriver::new());
    scripted(&w);
    started(&w, &listing("demo", vec![lib()]));
    let err = run(&w.cx, &listing("demo", vec![deploy()])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("o/lib"), "{}", err.error);
    run(&w.cx, &listing("demo", vec![lib()])).unwrap();
    run(&w.cx, &repo_req("demo")).unwrap();
}

#[test]
fn other_repos_need_a_primary_repo() {
    let w = world(FakeDriver::new());
    scripted(&w);
    let req = StartReq {
        repo: None,
        ..listing("demo", vec![lib()])
    };
    let err = run(&w.cx, &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

fn submodule_warnings(printed: [&str; 2]) -> Vec<String> {
    let w = world(FakeDriver::new().exec_stdout(printed));
    scripted(&w);
    run(&w.cx, &listing("demo", vec![lib()])).unwrap();
    w.cx.take_warnings()
        .into_iter()
        .filter(|w| w.code == "submodule")
        .map(|w| w.message)
        .collect()
}

#[test]
fn a_submodule_the_clone_left_out_warns_and_the_start_goes_on() {
    let warned = submodule_warnings(["moor.clone.submodule=vendor/engine: fatal: refused\n", ""]);
    assert_eq!(
        warned,
        ["submodule vendor/engine: fatal: refused; the Box runs without it"]
    );
}

#[test]
fn a_submodule_another_repo_left_out_warns_with_that_repo() {
    let warned = submodule_warnings(["", "moor.clone.submodule=native: fatal: refused\n"]);
    assert_eq!(
        warned,
        ["submodule o/lib:native: fatal: refused; the Box runs without it"]
    );
}

#[test]
fn a_clone_with_every_submodule_warns_of_none() {
    assert!(submodule_warnings(["", ""]).is_empty());
}
