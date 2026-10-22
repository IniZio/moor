//! Tests for the Box exec environment.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule, GITHUB_SECRET};
use moor_core::config::{parse_user, BoxSettings, Flags, UserConfig};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::exec_env::Purpose;
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::user::{Identity, UserId};
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{forge_routed, prepare, BoxProxy, Site};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::proxy_host::{FakeProxyHost, ProxyHostCall};
use crate::testing::surface::FakeSurface;

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    handle: BoxHandle,
    workdir: BoxPath,
}

fn rig(dir: &Path) -> Rig {
    rig_with(dir, UserConfig::default())
}

fn rig_with(dir: &Path, config: UserConfig) -> Rig {
    rig_driver(dir, config, FakeDriver::new())
}

fn rig_driver(dir: &Path, config: UserConfig, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
    let cx = Cx::fake_with(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        config,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: BoxId::new(1),
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
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
    fn site<'a>(&'a self, workdir: Option<&'a BoxPath>) -> Site<'a> {
        Site {
            handle: &self.handle,
            provider: "p",
            workdir,
        }
    }
}

fn cfg() -> BoxSettings {
    let mut c = BoxSettings::resolve(&UserConfig::default(), None, &Flags::default());
    c.env = [("REPO".to_owned(), "1".to_owned())].into();
    c
}

fn boxed_proxy() -> BoxProxy {
    BoxProxy {
        addr: ProxyAddr {
            box_port: 4000,
            ca_pem: "CA-PEM".to_owned(),
            box_env: vec![("RULE".to_owned(), "x".to_owned())],
            forge: false,
            direct: Vec::new(),
        },
        rule_env: vec![("RULE".to_owned(), "x".to_owned())],
    }
}

#[test]
fn a_proxy_runs_no_box_step_and_the_prelude_writes_the_ca_and_exports_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let calls = r.driver.calls().len();
    let p = boxed_proxy();
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir)),
        Some(&p),
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    assert_eq!(r.driver.calls().len(), calls, "{:?}", r.driver.calls());
    assert_eq!(
        e.vars(),
        [
            ("REPO".to_owned(), "1".to_owned()),
            ("RULE".to_owned(), "x".to_owned())
        ]
    );
    let script = e.argv(vec!["true".to_owned()]).join(" ");
    assert!(
        script.contains("HTTPS_PROXY=\"http://127.0.0.1:4000\""),
        "{script}"
    );
    assert!(
        script.contains("moor_ca='CA-PEM'; mkdir -p \"$HOME/.moor-proxy\""),
        "{script}"
    );
}

#[test]
fn a_proxy_keeps_credential_like_and_reserved_repo_env_without_warning() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let mut c = cfg();
    c.env.insert("STRIPE_TEST_KEY".to_owned(), "sk".to_owned());
    c.env.insert("NODE_OPTIONS".to_owned(), "--x".to_owned());
    let p = boxed_proxy();
    let e = prepare(&r.cx, &r.site(Some(&r.workdir)), Some(&p), &c, Purpose::Run).unwrap();
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
    let e = prepare(&r.cx, &r.site(Some(&r.workdir)), None, &cfg(), Purpose::Run).unwrap();
    assert_eq!(e.vars(), vec![("REPO".to_owned(), "1".to_owned())]);
    assert!(r.driver.execs().is_empty());
}

#[test]
fn no_proxy_wraps_the_argv_with_only_the_image_path_line() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let e = prepare(&r.cx, &r.site(Some(&r.workdir)), None, &cfg(), Purpose::Run).unwrap();
    let argv = e.argv(vec!["true".to_owned()]);
    assert_eq!(argv.get(..2).unwrap(), ["sh", "-c"]);
    assert!(argv.get(2).unwrap().contains("/etc/environment"));
    assert!(!argv.get(2).unwrap().contains("PROXY"));
    assert_eq!(argv.get(3..).unwrap(), ["sh", "true"]);
}

#[test]
fn no_recorded_workdir_keeps_the_driver_default_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let r = rig(dir.path());
    let e = prepare(&r.cx, &r.site(None), None, &cfg(), Purpose::Run).unwrap();
    assert_eq!(e.cwd(), None);
}

#[test]
fn the_providers_strip_env_is_unset_first_and_absent_from_the_vars() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = "drivers: {p: {strip_env: [REPO]}}";
    let r = rig_with(dir.path(), parse_user("config.yaml", yaml).unwrap());
    let e = prepare(&r.cx, &r.site(Some(&r.workdir)), None, &cfg(), Purpose::Run).unwrap();
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
    let e = prepare(&r.cx, &r.site(None), None, &cfg(), Purpose::Run).unwrap();
    assert!(e.env.unset.is_empty());
    assert_eq!(e.vars(), vec![("REPO".to_owned(), "1".to_owned())]);
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
    let rules = [rule_with("other"), rule_with(GITHUB_SECRET)];
    assert!(forge_routed(&rules));
}

#[test]
fn rules_without_a_forge_credential_do_not() {
    assert!(!forge_routed(&[rule_with("other")]));
    assert!(!forge_routed(&[]));
}

#[test]
fn ensure_plans_the_proxy_for_the_user_of_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProxyHost::new();
    let mut r = rig(dir.path());
    r.cx = r.cx.with_proxy_host(Arc::new(fake.clone()));
    let store = r.cx.store().unwrap();
    let (second, _) = store.enroll(&Identity::local(2000)).unwrap();
    assert_ne!(second.get(), 1, "the Box's user is not the first user");
    let new = NewBox {
        name: BoxName::parse_allow_bare("second-users-box").unwrap(),
        repo: "r".into(),
    };
    let id = store
        .apply(Decision::insert(new).requester(second))
        .unwrap()
        .box_id;
    super::proxy(&r.cx, id, Some(BoxHandle::new("h")), dir.path(), &cfg()).unwrap();
    let users: Vec<UserId> = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            ProxyHostCall::Ensure(spec) => Some(spec.user),
            _ => None,
        })
        .collect();
    assert_eq!(users, [second]);
}
