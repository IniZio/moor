//! Tests for `retry`.

use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxId, BoxName, BoxPath, BoxReq, ErrorKind, StartReq};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct World {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
}

fn name() -> BoxName {
    BoxName::parse_allow_bare("demo").unwrap()
}

const fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// A world whose forge serves a repo with `.moor.yaml` text `yaml`.
fn world(yaml: &str, driver: FakeDriver) -> World {
    let dir = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=4).fold(driver, |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
    let forge = Arc::new(FakeForge::new());
    forge.answer(ForgeRepo::new("o", "n"), Ok(repo_at("abc123", Some(yaml))));
    let cx = Cx::fake_with(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
    )
    .with_forge(forge);
    World {
        _dir: dir,
        cx,
        driver,
    }
}

/// Starts `demo` with no agent, whether or not the start succeeds, and returns its id.
fn started(w: &World) -> BoxId {
    let req = StartReq {
        name: name(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: true,
        resume: false,
        repo: Some(ForgeRepo::new("o", "n")),
        parent: None,
        also: None,
        identity: None,
    };
    let _ = crate::ops::start::run(&w.cx, &req);
    w.cx.store()
        .unwrap()
        .open_box_by_name(&name())
        .unwrap()
        .unwrap()
        .id
}

fn req() -> BoxReq {
    BoxReq { box_name: name() }
}

fn row(w: &World, id: BoxId) -> crate::store::BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

#[test]
fn retry_after_a_failed_clone_runs_the_steps_again_and_clears_the_failure() {
    let w = world(
        "setup: [a]\n",
        FakeDriver::new().exec_results([Ok(exit(1))]),
    );
    let id = started(&w);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::SetupFailed);
    assert!(row(&w, id).setup_failed_at.is_some());
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.attached);
    assert!(v.setup.ran && v.setup.ok);
    let after = row(&w, id);
    assert_eq!(after.lifecycle, BoxLifecycle::Ready);
    assert!(after.setup_failed_at.is_none());
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn retry_of_a_box_that_did_not_fail_setup_is_refused() {
    let w = world("setup: [a]\n", FakeDriver::new());
    let id = started(&w);
    let before = row(&w, id).lifecycle;
    assert_ne!(before, BoxLifecycle::SetupFailed);
    let e = run(&w.cx, &req()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(
        e.error.to_string().starts_with("not_setup_failed: "),
        "{}",
        e.error
    );
    assert_eq!(row(&w, id).lifecycle, before);
}

#[test]
fn retry_of_an_unknown_box_says_there_is_none() {
    let w = world("setup: [a]\n", FakeDriver::new());
    let e = run(&w.cx, &req()).unwrap_err();
    assert!(e.error.to_string().contains("no open Box demo"));
}

#[test]
fn retry_where_the_machine_is_gone_acquires_a_new_one_and_runs_setup() {
    let w = world(
        "setup: [a]\n",
        FakeDriver::new().exec_results([Ok(exit(1))]),
    );
    let id = started(&w);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let v = run(&w.cx, &req()).unwrap();
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
    let acquires = w.driver.calls().iter().filter(|c| *c == "acquire").count();
    assert_eq!(acquires, 2);
}
