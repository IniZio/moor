use std::sync::Arc;

use moor_core::config::{Flags, Layer};
use moor_core::domain::decision::Decision;
use moor_core::seam::forge::ForgeRepo;
use moor_core::BoxName;

use super::{repo_config_of, settings_of};
use crate::cx::Cx;
use crate::store::BoxRow;
use crate::testing::cx::{box_repo, new_box, new_box_of, store_config};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

fn cx(dir: &std::path::Path) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
}

/// The row of the Box `insert` makes.
fn row(cx: &Cx, insert: Decision) -> BoxRow {
    let store = cx.store().unwrap();
    let id = store.apply(insert).unwrap().box_id;
    store.box_row(id).unwrap().unwrap()
}

fn repo() -> ForgeRepo {
    ForgeRepo::new("o", "n")
}

fn name() -> BoxName {
    BoxName::parse_allow_bare("t").unwrap()
}

#[test]
fn flag_model_resolves_and_repo_has_no_model() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    store_config(cx.store().unwrap(), &repo(), "setup: ['true']\n");
    let row = row(&cx, new_box_of(name(), box_repo()));

    assert!(settings_of(&cx, &row, &Flags::default())
        .unwrap()
        .model
        .is_none());

    let flags = Flags {
        model: Some("flag-m".into()),
        ..Flags::default()
    };
    let m = settings_of(&cx, &row, &flags).unwrap().model.unwrap();
    assert_eq!((m.value.as_str(), m.source), ("flag-m", Layer::Flag));
}

#[test]
fn no_repo_config_resolves_defaults() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let none = BoxName::parse_allow_bare("u").unwrap();
    for insert in [new_box(none), new_box_of(name(), box_repo())] {
        let row = row(&cx, insert);
        let s = settings_of(&cx, &row, &Flags::default()).unwrap();
        assert!(s.model.is_none());
        assert!(s.setup.is_empty());
    }
}

#[test]
fn an_edit_of_the_stored_config_reaches_a_running_box_at_once() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let store = cx.store().unwrap();
    store_config(store, &repo(), "env: {A: first}\nsetup: [build]\n");
    let row = row(&cx, new_box_of(name(), box_repo()));
    let s = settings_of(&cx, &row, &Flags::default()).unwrap();
    assert_eq!(s.env.get("A").map(String::as_str), Some("first"));
    assert_eq!(s.setup, ["build"]);

    store
        .set_repo_config(&repo(), "env: {A: edited}\n", "operator", Some(1))
        .unwrap();
    let s = settings_of(&cx, &row, &Flags::default()).unwrap();
    assert_eq!(s.env.get("A").map(String::as_str), Some("edited"));
    assert!(s.setup.is_empty(), "an edit replaces the config whole");

    store
        .set_repo_config(&repo(), "env: {A: again}\n", "operator", Some(2))
        .unwrap();
    let s = settings_of(&cx, &row, &Flags::default()).unwrap();
    assert_eq!(s.env.get("A").map(String::as_str), Some("again"));
}

#[test]
fn a_config_stored_after_a_box_started_reaches_it() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let row = row(&cx, new_box_of(name(), box_repo()));
    assert!(settings_of(&cx, &row, &Flags::default())
        .unwrap()
        .setup
        .is_empty());
    store_config(cx.store().unwrap(), &repo(), "setup: [fresh]\n");
    let s = settings_of(&cx, &row, &Flags::default()).unwrap();
    assert_eq!(s.setup, ["fresh"]);
}

#[test]
fn a_stored_config_that_does_not_parse_names_the_repo_and_revision() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    store_config(cx.store().unwrap(), &repo(), "bogus: 1\n");
    let err = repo_config_of(&cx, Some(&repo()), None).unwrap_err();
    assert!(
        err.error
            .to_string()
            .contains("o/n:repo config (revision 1)"),
        "{err:?}"
    );
}
