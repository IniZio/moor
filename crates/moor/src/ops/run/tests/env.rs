//! Tests for the environment and working directory `run` gives the command.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxyHostError;

use super::{command, ready, req, run};
use crate::testing::proxy_host::{FakeProxyHost, FAKE_NONCE};

#[test]
fn run_gets_the_repo_env_and_proxy_variables() {
    let f = ready();
    std::fs::write(f.dir.path().join(".moor.yaml"), "env: {K: v}\n").unwrap();
    let mut r = req(&["echo hi"]);
    r.trust_moor_yaml = true;
    run(&f.cx, &r).unwrap();
    let (_, spec, deadline) = f.driver.execs().remove(0);
    assert_eq!(spec.env.first(), Some(&("K".to_owned(), "v".to_owned())));
    assert_eq!(spec.cwd.unwrap().as_str(), "/work/cbx_1/repo");
    assert_eq!(deadline, Deadline(Duration::from_hours(24)));
    let script = spec.argv.get(2).unwrap();
    assert_eq!(spec.argv.first().map(String::as_str), Some("sh"));
    assert!(script.contains("export HTTPS_PROXY="), "{script}");
    assert!(
        !script.contains(&*String::from_utf8_lossy(FAKE_NONCE)),
        "{script}"
    );
    assert_eq!(command(&spec.argv), ["sh", "-c", "echo hi"]);
}

#[test]
fn box_without_a_recorded_workdir_runs_in_the_default_directory() {
    let f = ready();
    let unrecorded = Decision::new(f.id).set(Fields {
        box_workdir: Field::Set(String::new()),
        ..Fields::default()
    });
    f.cx.store().unwrap().apply(unrecorded).unwrap();
    let v = run(&f.cx, &req(&["true"])).unwrap();
    assert!(v.ran);
    let (_, spec, _) = f.driver.execs().remove(0);
    assert_eq!(spec.cwd, None);
}

const REPO_ENV: &str =
    "env: {NODE_OPTIONS: '--max-old-space-size=4096', STRIPE_TEST_KEY: sk_test_x}\n";

fn has_env(env: &[(String, String)], k: &str, v: &str) -> bool {
    env.iter().any(|(ek, ev)| ek == k && ev == v)
}

fn assert_full_repo_env(env: &[(String, String)]) {
    assert!(
        has_env(env, "NODE_OPTIONS", "--max-old-space-size=4096"),
        "{env:?}"
    );
    assert!(has_env(env, "STRIPE_TEST_KEY", "sk_test_x"), "{env:?}");
}

#[test]
fn run_with_a_proxy_gets_reserved_and_credential_like_repo_env() {
    let f = ready();
    std::fs::write(f.dir.path().join(".moor.yaml"), REPO_ENV).unwrap();
    let mut r = req(&["echo hi"]);
    r.trust_moor_yaml = true;
    run(&f.cx, &r).unwrap();
    let (_, spec, _) = f.driver.execs().remove(0);
    assert!(spec.argv.get(2).unwrap().contains("export HTTPS_PROXY="));
    assert_full_repo_env(&spec.env);
}

#[test]
fn run_without_a_proxy_gets_reserved_and_credential_like_repo_env() {
    let mut f = ready();
    f.cx = f.cx.with_proxy_host(Arc::new(
        FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("down".into())),
    ));
    std::fs::write(f.dir.path().join(".moor.yaml"), REPO_ENV).unwrap();
    let mut r = req(&["echo hi"]);
    r.trust_moor_yaml = true;
    run(&f.cx, &r).unwrap();
    let (_, spec, _) = f.driver.execs().remove(0);
    assert!(!spec.argv.get(2).unwrap().contains("export HTTPS_PROXY="));
    assert_full_repo_env(&spec.env);
}
