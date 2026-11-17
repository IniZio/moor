//! A Box's override layers over its repo's config, is hot through `box-config-set`, and touches
//! no other Box.

use std::sync::Arc;

use moor_core::config::Flags;
use moor_core::domain::decision::Decision;
use moor_core::request::BoxConfigSetReq;
use moor_core::seam::forge::ForgeRepo;
use moor_core::BoxName;

use super::{check, set};
use crate::cx::Cx;
use crate::ops::common::facts::settings_of;
use crate::store::BoxRow;
use crate::testing::cx::{box_repo, new_box_of, store_config};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

fn cx(dir: &std::path::Path) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
}

/// Stores `yaml` as the config of the repo `o/n` every test Box clones.
fn repo_config(cx: &Cx, yaml: &str) {
    store_config(cx.store().unwrap(), &ForgeRepo::new("o", "n"), yaml);
}

fn named(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

/// The row of the Box `insert` makes.
fn row(cx: &Cx, insert: Decision) -> BoxRow {
    let store = cx.store().unwrap();
    let id = store.apply(insert).unwrap().box_id;
    store.box_row(id).unwrap().unwrap()
}

fn env_of(cx: &Cx, row: &BoxRow) -> Vec<(String, String)> {
    let s = settings_of(cx, row, &Flags::default()).unwrap();
    s.env.into_iter().collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn put(cx: &Cx, name: &str, config: Option<&str>) {
    set(
        cx,
        &BoxConfigSetReq {
            box_name: named(name),
            config: config.map(str::to_owned),
        },
    )
    .unwrap();
}

#[test]
fn a_box_override_wins_over_the_repo_and_leaves_other_boxes_alone() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    repo_config(&cx, "env: {A: repo}\nsetup: [build]\n");
    let first = row(&cx, new_box_of(named("a"), box_repo()));
    let second = row(&cx, new_box_of(named("b"), box_repo()));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "repo")]));

    put(&cx, "a", Some("env: {A: box, B: box}\n"));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "box"), ("B", "box")]));
    assert_eq!(env_of(&cx, &second), pairs(&[("A", "repo")]));
    let s = settings_of(&cx, &first, &Flags::default()).unwrap();
    assert_eq!(s.setup, ["build"], "a key the override leaves out stays");
}

#[test]
fn a_set_is_hot_and_a_blank_set_removes_the_override() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    repo_config(&cx, "env: {A: repo}\n");
    let first = row(&cx, new_box_of(named("a"), box_repo()));
    put(&cx, "a", Some("env: {B: one}\n"));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "repo"), ("B", "one")]));
    put(&cx, "a", Some("env: {B: two}\n"));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "repo"), ("B", "two")]));
    put(&cx, "a", Some("  \n"));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "repo")]));
    put(&cx, "a", Some("env: {B: three}\n"));
    put(&cx, "a", None);
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "repo")]));
}

#[test]
fn an_override_applies_where_the_repo_has_no_config() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let first = row(&cx, new_box_of(named("a"), box_repo()));
    put(&cx, "a", Some("env: {A: box}\nsetup: [x]\n"));
    assert_eq!(env_of(&cx, &first), pairs(&[("A", "box")]));
}

#[test]
fn an_override_that_does_not_parse_or_is_too_long_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let _ = row(&cx, new_box_of(named("a"), box_repo()));
    assert!(check(Some("bogus: 1\n")).is_err());
    assert!(check(Some(&"# x\n".repeat(20_000))).is_err());
    assert!(check(Some("env: {A: b}\n")).is_ok());
    assert!(check(None).is_ok());
    let bad = BoxConfigSetReq {
        box_name: named("a"),
        config: Some("bogus: 1\n".to_owned()),
    };
    assert!(set(&cx, &bad).is_err());
}

#[test]
fn a_box_that_is_not_open_has_nothing_to_set() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let req = BoxConfigSetReq {
        box_name: named("nope"),
        config: Some("env: {A: b}\n".to_owned()),
    };
    assert!(set(&cx, &req).is_err());
}
