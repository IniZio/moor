use std::sync::Arc;

use moor_core::config::{Flags, Layer};
use moor_core::domain::decision::Decision;
use moor_core::BoxName;

use super::{repo_config_of, settings_of};
use crate::cx::Cx;
use crate::store::BoxRow;
use crate::testing::cx::{box_repo, new_box, new_box_of};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

fn cx(dir: &std::path::Path) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
}

/// The row of the Box `insert` makes.
fn row(cx: &Cx, insert: Decision) -> BoxRow {
    let store = cx.store().unwrap();
    let id = store.apply(insert).unwrap().box_id;
    store.box_row(id).unwrap().unwrap()
}

fn name() -> BoxName {
    BoxName::parse_allow_bare("t").unwrap()
}

#[test]
fn flag_model_resolves_and_repo_has_no_model() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let row = row(&cx, new_box_of(name(), box_repo(Some("setup: ['true']\n"))));

    assert!(settings_of(&cx, &row, &Flags::default())
        .unwrap()
        .model
        .is_none());

    let flags = Flags {
        model: Some("flag-m".into()),
    };
    let m = settings_of(&cx, &row, &flags).unwrap().model.unwrap();
    assert_eq!((m.value.as_str(), m.source), ("flag-m", Layer::Flag));
}

#[test]
fn stored_repo_config_is_what_settings_read() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let row = row(
        &cx,
        new_box_of(name(), box_repo(Some("setup: ['echo hi']\n"))),
    );
    let s = settings_of(&cx, &row, &Flags::default()).unwrap();
    assert_eq!(s.setup, ["echo hi"]);
}

#[test]
fn no_repo_config_resolves_defaults() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let none = BoxName::parse_allow_bare("u").unwrap();
    for insert in [new_box(none), new_box_of(name(), box_repo(None))] {
        let row = row(&cx, insert);
        let s = settings_of(&cx, &row, &Flags::default()).unwrap();
        assert!(s.model.is_none());
        assert!(s.setup.is_empty());
    }
}

#[test]
fn bad_repo_config_is_a_failed_command_naming_the_repo_and_base() {
    let repo = box_repo(Some("agent: {model: m}\n"));
    let err = repo_config_of(Some(&repo), Some("abc123")).unwrap_err();
    assert!(
        err.error.to_string().contains("o/n@abc123:.moor.yaml"),
        "{err:?}"
    );
}
