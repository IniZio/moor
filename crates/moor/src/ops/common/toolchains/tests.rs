//! Tests for the host toolchains a repo selects.

use std::sync::Arc;

use moor_core::config::{parse_user, Flags, TaskSettings, UserConfig};
use moor_core::domain::decision::{Decision, NewTask};
use moor_core::{TaskId, TaskName};

use super::{env_and_bin, mac_grants, plan_links, selected};
use crate::cx::Cx;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const HOST: &str = "toolchains:\n  mise: {root: /Users/me/.local/share/mise, env: {MISE_DATA_DIR: \"{root}\"}, bin: [installs/node/22/bin]}\n  rbenv: {root: /Users/me/.rbenv, bin: [shims]}\n";

fn cx(dir: &std::path::Path) -> Cx {
    let cx = Cx::fake_with(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
        parse_user("c.yaml", HOST).unwrap(),
    );
    cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    cx
}

fn cfg(names: &[&str]) -> TaskSettings {
    let mut c = TaskSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.repo_box.toolchains = names.iter().map(|n| (*n).to_owned()).collect();
    c
}

#[test]
fn mac_grants_carry_the_selected_toolchains_in_the_order_named() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let g = mac_grants(&cx, &cfg(&["rbenv", "mise"])).unwrap();
    let names: Vec<&str> = g.toolchains.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["rbenv", "mise"]);
}

#[test]
fn mac_grants_refuse_an_unknown_toolchain() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let e = mac_grants(&cx, &cfg(&["nope"])).unwrap_err();
    assert!(e.to_string().contains("unknown toolchain \"nope\""), "{e}");
}

#[test]
fn no_selection_grants_no_toolchains() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    assert!(mac_grants(&cx, &cfg(&[])).unwrap().toolchains.is_empty());
}

#[test]
fn env_and_bin_flatten_every_toolchain_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let tc = selected(&cx, &cfg(&["mise", "rbenv"])).unwrap();
    let (env, bin) = env_and_bin(&tc);
    assert_eq!(
        env,
        [(
            "MISE_DATA_DIR".to_owned(),
            "/Users/me/.local/share/mise".to_owned()
        )]
    );
    assert_eq!(
        bin,
        [
            "/Users/me/.local/share/mise/installs/node/22/bin",
            "/Users/me/.rbenv/shims"
        ]
    );
}

#[test]
fn plan_links_plans_one_lease_per_root_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let tc = selected(&cx, &cfg(&["mise", "rbenv"])).unwrap();
    let store = cx.store().unwrap();
    let first = plan_links(store, TaskId::new(1), &tc).unwrap();
    assert_eq!(plan_links(store, TaskId::new(1), &tc).unwrap(), first);
    let rows = store.leases(TaskId::new(1)).unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows
        .iter()
        .all(|r| r.kind == LeaseKind::ToolchainLink && r.state == LeaseState::Planned));
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["/Users/me/.local/share/mise", "/Users/me/.rbenv"]);
}
