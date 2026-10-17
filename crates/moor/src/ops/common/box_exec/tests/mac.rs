//! Tests for the macOS Box exec environment.

use std::path::Path;

use moor_core::config::parse_user;
use moor_core::domain::decision::{Decision, NewTask};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::ExecOutcome;
use moor_core::{BoxPath, TaskId, TaskName};

use super::{argvs, boxed_proxy, cfg, rig_driver, Rig};
use crate::ops::common::box_exec::{prepare, tag_nonce, BoxExec};
use crate::store::lease::{LeaseKind, LeaseState};
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
    let [prep, mkdir] = argvs(&r).try_into().unwrap();
    assert_eq!(prep.get(..2), Some(&["sh".to_owned(), "-c".to_owned()][..]));
    assert!(prep.last().unwrap().contains("/Users/box/moor/demo/launch"));
    assert_eq!(mkdir.first().map(String::as_str), Some("mkdir"));
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

#[test]
fn the_tag_nonce_is_sixteen_lowercase_hex_so_box_release_matches_the_tag_anchored() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    for _ in 0..8 {
        let n = tag_nonce(r.cx.rng());
        assert_eq!(n.len(), 16, "{n}");
        assert!(
            n.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{n}"
        );
    }
}

const HOST: &str = "toolchains:\n  mise: {root: /Users/me/.local/share/mise, env: {MISE_DATA_DIR: \"{root}\"}, bin: [installs/node/22/bin]}\n";

#[test]
fn the_selected_toolchains_reach_the_env_and_the_path_and_their_links_are_held() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig_driver(
        dir.path(),
        parse_user("c.yaml", HOST).unwrap(),
        FakeDriver::new(),
    );
    r.cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    let mut c = cfg();
    c.repo_box.toolchains = vec!["mise".to_owned()];
    let workdir = BoxPath::restore(WORKDIR).unwrap();
    let e = prepare(
        &r.cx,
        &r.site(Some(&workdir), dir.path()),
        None,
        &c,
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(
        var(&e, "MISE_DATA_DIR").as_deref(),
        Some("/Users/me/.local/share/mise")
    );
    let prelude = e.env.prelude.join("\n");
    assert!(
        prelude.contains("'/Users/me/.local/share/mise/installs/node/22/bin':"),
        "{prelude}"
    );
    let rows = r.cx.store().unwrap().leases(TaskId::new(1)).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = rows.first().unwrap();
    assert_eq!(row.kind, LeaseKind::ToolchainLink);
    assert_eq!(row.key, "/Users/me/.local/share/mise");
    assert_eq!(row.state, LeaseState::Held);
}

#[test]
fn an_unknown_toolchain_fails_the_exec_as_a_config_error_before_the_box_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    let mut c = cfg();
    c.repo_box.toolchains = vec!["nope".to_owned()];
    let workdir = BoxPath::restore(WORKDIR).unwrap();
    let got = prepare(
        &r.cx,
        &r.site(Some(&workdir), dir.path()),
        None,
        &c,
        Purpose::Run,
    );
    let e = got.err().unwrap();
    assert!(
        e.starts_with(".moor.yaml:box.toolchains[0]:") && e.contains("unknown toolchain \"nope\""),
        "{e}"
    );
    assert!(argvs(&r).is_empty());
}

fn hold_sim_device(r: &Rig, udid: &str) {
    let store = r.cx.store().unwrap();
    let id = store
        .plan_lease(TaskId::new(1), LeaseKind::SimDevice, udid)
        .unwrap();
    assert!(store.hold_lease(id).unwrap());
}

#[test]
fn a_task_holding_a_simulator_device_gets_its_udid_and_set_in_the_exec_env() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    hold_sim_device(&r, "UDID-1");
    let e = run(&r, dir.path(), WORKDIR, false);
    assert_eq!(var(&e, "MOOR_SIM_UDID").as_deref(), Some("UDID-1"));
    assert_eq!(
        var(&e, "MOOR_SIM_SET").as_deref(),
        Some("/Users/box/moor/demo/sim")
    );
}

#[test]
fn a_task_without_a_simulator_device_gets_neither_simulator_var() {
    let dir = tempfile::tempdir().unwrap();
    let r = mac_rig(dir.path());
    let e = run(&r, dir.path(), WORKDIR, false);
    assert!(var(&e, "MOOR_SIM_UDID").is_none());
    assert!(var(&e, "MOOR_SIM_SET").is_none());
}
