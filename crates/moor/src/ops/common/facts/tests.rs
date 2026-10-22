use std::path::Path;
use std::sync::Arc;

use moor_core::config::{Flags, Layer};
use moor_core::seam::proc::Tool;

use super::settings;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

/// Context whose main checkout is `main`.
fn cx(dir: &Path, main: &Path) -> Cx {
    let proc = ScriptedProc::new();
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", main.display())),
    );
    Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(proc),
    )
}

#[test]
fn flag_model_resolves_and_repo_has_no_model() {
    let d = tempfile::tempdir().unwrap();
    let main = d.path().join("main");
    let wt = d.path().join("wt");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(main.join(".moor.yaml"), "setup: ['true']\n").unwrap();
    let cx = cx(d.path(), &main);

    assert!(settings(&cx, &wt, &Flags::default())
        .unwrap()
        .model
        .is_none());

    let flags = Flags {
        model: Some("flag-m".into()),
    };
    let m = settings(&cx, &wt, &flags).unwrap().model.unwrap();
    assert_eq!((m.value.as_str(), m.source), ("flag-m", Layer::Flag));
}

#[test]
fn no_repo_config_resolves_defaults() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path(), d.path());
    let s = settings(&cx, d.path(), &Flags::default()).unwrap();
    assert!(s.model.is_none());
}

#[test]
fn bad_repo_config_is_a_failed_command() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(".moor.yaml"), "agent: {model: m}\n").unwrap();
    let cx = cx(d.path(), d.path());
    assert!(settings(&cx, d.path(), &Flags::default()).is_err());
}
