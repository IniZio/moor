//! Tests for the macOS Box exec environment.

use std::path::Path;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::ExecOutcome;
use moor_core::{BoxPath, TaskName};

use super::{argvs, boxed_proxy, cfg, rig_driver, Rig};
use crate::ops::common::box_exec::{prepare, BoxExec};
use crate::testing::driver::FakeDriver;

const WORKDIR: &str = "/Users/box/moor/demo/work/repo";

fn mac_rig(dir: &Path) -> Rig {
    rig_with_driver(dir, FakeDriver::new())
}

fn rig_with_driver(dir: &Path, driver: FakeDriver) -> Rig {
    let r = rig_driver(dir, crate::testing::cx::fake_config(), driver);
    r.cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    r
}

fn run(r: &Rig, dir: &Path, workdir: &str, proxied: bool) -> BoxExec {
    let workdir = BoxPath::restore(workdir).unwrap();
    let p = boxed_proxy();
    prepare(
        &r.cx,
        &r.site(Some(&workdir), dir),
        proxied.then_some(&p),
        &cfg(),
        Purpose::Run,
    )
    .unwrap()
}

fn var(e: &BoxExec, name: &str) -> Option<String> {
    e.vars()
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
}

#[test]
fn a_mac_exec_without_a_proxy_gets_the_task_env_and_prepares_the_layout_first() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    let e = run(&r, dir.path(), WORKDIR, false);
    assert_eq!(
        var(&e, "HOME").as_deref(),
        Some("/Users/box/moor/demo/home")
    );
    let tag = var(&e, "MOOR_BOX_TAG").unwrap();
    assert!(tag.starts_with("demo-"), "{tag}");
    let [prep] = argvs(&r).try_into().unwrap();
    assert_eq!(prep.get(..2), Some(&["sh".to_owned(), "-c".to_owned()][..]));
    assert!(prep.last().unwrap().contains("/Users/box/moor/demo/launch"));
    assert!(!r.driver.calls().iter().any(|c| c.starts_with("peek_file")));
}

#[test]
fn a_mac_exec_with_a_proxy_prepares_the_layout_before_staging_and_never_peeks_the_environ() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    let e = run(&r, dir.path(), WORKDIR, true);
    assert_eq!(
        var(&e, "HOME").as_deref(),
        Some("/Users/box/moor/demo/home")
    );
    assert!(var(&e, "MOOR_BOX_TAG").unwrap().starts_with("demo-"));
    let [prep, mkdir] = argvs(&r).try_into().unwrap();
    assert_eq!(prep.get(..2), Some(&["sh".to_owned(), "-c".to_owned()][..]));
    assert_eq!(mkdir.first().map(String::as_str), Some("mkdir"));
    assert!(!r.driver.calls().iter().any(|c| c.starts_with("peek_file")));
}

#[test]
fn a_workdir_outside_the_layout_runs_as_on_linux() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    let e = run(&r, dir.path(), "/srv/other/work", false);
    assert!(var(&e, "MOOR_BOX_TAG").is_none());
    assert!(argvs(&r).is_empty());
}

#[test]
fn a_failed_layout_prepare_names_the_step_and_does_not_build_the_env() {
    let dir = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let r = rig_with_driver(dir.path(), driver);
    let workdir = BoxPath::restore(WORKDIR).unwrap();
    let got = prepare(
        &r.cx,
        &r.site(Some(&workdir), dir.path()),
        None,
        &cfg(),
        Purpose::Run,
    );
    let e = got.err().unwrap();
    assert!(e.contains("prepare the Task directories"), "{e}");
}
