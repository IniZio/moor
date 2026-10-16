//! Tests for the Box exec environment.

use std::path::Path;
use std::sync::Arc;

use moor_core::config::{parse_user, Flags, TaskSettings, UserConfig};
use moor_core::exec_env::{Purpose, CA_FILE, NONCE_FILE};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::secret::Secret;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::{prepare, BoxProxy, Site};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    handle: BoxHandle,
    workdir: BoxPath,
}

fn rig(dir: &Path) -> Rig {
    rig_with(dir, crate::testing::cx::fake_config())
}

fn rig_with(dir: &Path, config: UserConfig) -> Rig {
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake_with(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
        config,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: TaskId::new(1),
            name: TaskName::parse("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    Rig {
        cx,
        driver,
        handle: got.handle,
        workdir: got.workdir,
    }
}

impl Rig {
    fn site<'a>(&'a self, workdir: Option<&'a BoxPath>, worktree: &'a Path) -> Site<'a> {
        Site {
            handle: &self.handle,
            provider: "p",
            workdir,
            worktree,
        }
    }
}

fn cfg() -> TaskSettings {
    let mut c = TaskSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.env = [("REPO".to_owned(), "1".to_owned())].into();
    c
}

fn boxed_proxy() -> BoxProxy {
    BoxProxy {
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret".to_vec()),
            ca_pem: "CA-PEM".to_owned(),
            box_env: vec![("RULE".to_owned(), "x".to_owned())],
        },
        origin: None,
    }
}

#[test]
fn a_proxy_stages_the_nonce_and_ca_and_the_prelude_exports_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let p = boxed_proxy();
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        Some(&p),
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    let staged = e.staged.clone().unwrap();
    assert!(staged.as_str().starts_with("/tmp/moor-exec-"));
    let peek = |f: &str| {
        let path = BoxPath::under(staged.as_str(), f).unwrap();
        r.driver.peek_file(&r.handle, &path, dir.path()).unwrap()
    };
    assert_eq!(peek(NONCE_FILE).as_deref(), Some("s3cret"));
    assert_eq!(peek(CA_FILE).as_deref(), Some("CA-PEM"));
    assert_eq!(e.vars(), p.addr.box_env);
    let script = e.argv(vec!["true".to_owned()]).join(" ");
    assert!(script.contains("HTTPS_PROXY"), "{script}");
}

#[test]
fn no_proxy_gives_the_repo_env_and_no_proxy_variables() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        None,
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(e.vars(), vec![("REPO".to_owned(), "1".to_owned())]);
    assert!(r.driver.execs().is_empty());
}

#[test]
fn a_proxied_argv_removes_the_staged_directory_in_the_same_exec() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let p = boxed_proxy();
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        Some(&p),
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    let staged = e.staged.clone().unwrap();
    let script = e.argv(vec!["true".to_owned()]).join(" ");
    assert!(script.contains("trap"), "{script}");
    let trap = script
        .lines()
        .find(|l| l.contains("trap"))
        .unwrap_or_default();
    assert!(
        trap.contains("rm -rf") && trap.contains(staged.as_str()),
        "{script}"
    );
    assert!(r.driver.execs().is_empty());
}

#[test]
fn no_proxy_leaves_the_argv_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        None,
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(e.argv(vec!["true".to_owned()]), vec!["true".to_owned()]);
}

#[test]
fn no_recorded_workdir_keeps_the_driver_default_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let e = prepare(&r.cx, &r.site(None, dir.path()), None, &cfg(), Purpose::Run).unwrap();
    assert_eq!(e.cwd(), None);
}

#[test]
fn the_providers_strip_env_is_unset_first_and_absent_from_the_vars() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "drivers: {p: {strip_env: [REPO]}}";
    let r = rig_with(dir.path(), parse_user("config.yaml", yaml).unwrap());
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        None,
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(e.env.unset, ["REPO"]);
    assert!(e.vars().iter().all(|(n, _)| n != "REPO"));
    let argv = e.argv(vec!["true".to_owned()]);
    assert!(
        argv.get(2).is_some_and(|a| a.starts_with("unset REPO")),
        "{argv:?}"
    );
}

#[test]
fn another_providers_strip_env_is_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "drivers: {other: {strip_env: [REPO]}}";
    let r = rig_with(dir.path(), parse_user("config.yaml", yaml).unwrap());
    let e = prepare(&r.cx, &r.site(None, dir.path()), None, &cfg(), Purpose::Run).unwrap();
    assert!(e.env.unset.is_empty());
    assert_eq!(e.vars(), vec![("REPO".to_owned(), "1".to_owned())]);
}
