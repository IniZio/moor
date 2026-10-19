//! Tests for the Box exec environment.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule, FORGE_CREDENTIAL};
use moor_core::config::{parse_user, BoxSettings, Flags, UserConfig};
use moor_core::exec_env::{Purpose, CA_FILE, NONCE_FILE};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::secret::Secret;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{forge_routed, prepare, BoxProxy, Site};
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
    rig_driver(dir, config, FakeDriver::new())
}

/// A rig whose Box user's home is `/home/box`.
fn home_rig(dir: &Path) -> Rig {
    rig_driver(dir, crate::testing::cx::fake_config(), home_driver())
}

fn home_driver() -> FakeDriver {
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    FakeDriver::new().peek_text("box-1", &environ, "PATH=/bin\0HOME=/home/box\0")
}

fn rig_driver(dir: &Path, config: UserConfig, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
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
            task: BoxId::new(1),
            name: BoxName::parse_allow_bare("demo").unwrap(),
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

/// The argv of every Box exec so far, in order.
fn argvs(r: &Rig) -> Vec<Vec<String>> {
    r.driver
        .execs()
        .into_iter()
        .map(|(_, s, _)| s.argv)
        .collect()
}

fn cfg() -> BoxSettings {
    let mut c = BoxSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.env = [("REPO".to_owned(), "1".to_owned())].into();
    c
}

fn boxed_proxy() -> BoxProxy {
    BoxProxy {
        task: BoxId::new(1),
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret".to_vec()),
            ca_pem: "CA-PEM".to_owned(),
            box_env: vec![("RULE".to_owned(), "x".to_owned())],
            forge: false,
        },
        origin: None,
        rule_env: vec![("RULE".to_owned(), "x".to_owned())],
    }
}

#[test]
fn a_proxy_stages_the_nonce_and_ca_and_the_prelude_exports_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let r = home_rig(dir.path());
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
    assert!(staged.as_str().starts_with("/home/box/.moor-cred-1-"));
    let peek = |f: &str| {
        let path = BoxPath::under(staged.as_str(), f).unwrap();
        r.driver.peek_file(&r.handle, &path, dir.path()).unwrap()
    };
    assert_eq!(peek(NONCE_FILE).as_deref(), Some("s3cret"));
    assert_eq!(peek(CA_FILE).as_deref(), Some("CA-PEM"));
    assert_eq!(
        e.vars(),
        [
            ("REPO".to_owned(), "1".to_owned()),
            ("RULE".to_owned(), "x".to_owned())
        ]
    );
    let script = e.argv(vec!["true".to_owned()]).join(" ");
    assert!(script.contains("HTTPS_PROXY"), "{script}");
}

#[test]
fn a_proxy_keeps_credential_like_and_reserved_repo_env_without_warning() {
    let dir = tempfile::tempdir().unwrap();
    let r = home_rig(dir.path());
    let mut c = cfg();
    c.env.insert("STRIPE_TEST_KEY".to_owned(), "sk".to_owned());
    c.env.insert("NODE_OPTIONS".to_owned(), "--x".to_owned());
    let p = boxed_proxy();
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        Some(&p),
        &c,
        Purpose::Run,
    )
    .unwrap();
    let vars = e.vars();
    for name in ["STRIPE_TEST_KEY", "NODE_OPTIONS", "REPO", "RULE"] {
        assert!(vars.iter().any(|(k, _)| k == name), "{name} in {vars:?}");
    }
    assert!(r
        .cx
        .take_warnings()
        .iter()
        .all(|w| w.code != "agent_env_dropped"));
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
    let r = home_rig(dir.path());
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
    assert!(argvs(&r)
        .iter()
        .all(|a| a.first().map(String::as_str) != Some("rm")));
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

fn run_prepare(r: &Rig, dir: &Path, p: Option<&BoxProxy>) -> Result<super::BoxExec, String> {
    prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir),
        p,
        &cfg(),
        Purpose::Run,
    )
}

#[test]
fn the_staged_directory_is_private_to_the_box_user_and_named_with_64_random_bits() {
    let dir = tempfile::tempdir().unwrap();
    let r = home_rig(dir.path());
    let p = boxed_proxy();
    let e = run_prepare(&r, dir.path(), Some(&p)).unwrap();
    let staged = e.staged.clone().unwrap();
    let tail = staged
        .as_str()
        .strip_prefix("/home/box/.moor-cred-1-")
        .unwrap();
    assert!(
        tail.len() == 16 && tail.chars().all(|c| c.is_ascii_hexdigit()),
        "{tail}"
    );
    assert_eq!(argvs(&r), [["mkdir", "-m", "700", "--", staged.as_str()]]);
    for (_, path, ..) in r.driver.puts() {
        assert!(!path.as_str().starts_with("/tmp"), "{}", path.as_str());
    }
    let script = e.argv(vec!["true".to_owned()]).join(" ");
    assert!(!script.contains("/tmp"), "{script}");
}

#[test]
fn a_path_that_already_exists_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let driver = home_driver().exec_results([Ok(ExecOutcome::Exited { code: 1 })]);
    let r = rig_driver(dir.path(), crate::testing::cx::fake_config(), driver);
    let p = boxed_proxy();
    let err = run_prepare(&r, dir.path(), Some(&p)).err().unwrap();
    assert!(err.contains("already exists"), "{err}");
    let execs = argvs(&r);
    assert_eq!(execs.len(), 1);
    assert!(execs
        .iter()
        .all(|a| a.first().map(String::as_str) != Some("rm")));
    assert!(r.driver.puts().is_empty());
}

#[test]
fn a_timed_out_mkdir_says_so_instead_of_claiming_the_path_exists() {
    let dir = tempfile::tempdir().unwrap();
    let timeout = DriverError::Timeout(std::time::Duration::from_secs(30));
    let want = timeout.to_string();
    let driver = home_driver().exec_results([Err(timeout)]);
    let r = rig_driver(dir.path(), crate::testing::cx::fake_config(), driver);
    let p = boxed_proxy();
    let err = run_prepare(&r, dir.path(), Some(&p)).err().unwrap();
    assert!(err.contains(&want), "{err}");
    assert!(!err.contains("already exists"), "{err}");
}

#[test]
fn a_staging_failure_removes_the_directory_and_returns_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let driver = home_driver().fail_put(DriverError::Unavailable("down".into()));
    let r = rig_driver(dir.path(), crate::testing::cx::fake_config(), driver);
    let p = boxed_proxy();
    let err = run_prepare(&r, dir.path(), Some(&p)).err().unwrap();
    assert!(err.contains("could not stage"), "{err}");
    let [mkdir, rm] = argvs(&r).try_into().unwrap();
    assert_eq!(rm, ["rm", "-rf", mkdir.last().unwrap().as_str()]);
}

#[test]
fn cleanup_removes_the_staged_directory_and_is_a_no_op_without_a_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let r = home_rig(dir.path());
    let p = boxed_proxy();
    let e = run_prepare(&r, dir.path(), Some(&p)).unwrap();
    let staged = e.staged.clone().unwrap();
    e.cleanup(&r.cx, &r.site(Some(&r.workdir), dir.path()));
    let [_, rm] = argvs(&r).try_into().unwrap();
    assert_eq!(rm, ["rm", "-rf", staged.as_str()]);

    let r = rig(dir.path());
    let e = run_prepare(&r, dir.path(), None).unwrap();
    e.cleanup(&r.cx, &r.site(Some(&r.workdir), dir.path()));
    assert!(r.driver.execs().is_empty());
}

fn rule_with(credential: &str) -> ResolvedRule {
    ResolvedRule {
        name: "r".to_owned(),
        hosts: vec!["api.github.com".to_owned()],
        credential: credential.to_owned(),
        inject: ResolvedInject::BasicAuth {
            user: "x".to_owned(),
        },
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    }
}

#[test]
fn a_forge_rule_marks_the_proxy_as_forge_routed() {
    let rules = [rule_with("other"), rule_with(FORGE_CREDENTIAL)];
    assert!(forge_routed(&rules));
}

#[test]
fn rules_without_a_forge_credential_do_not() {
    assert!(!forge_routed(&[rule_with("other")]));
    assert!(!forge_routed(&[]));
}
