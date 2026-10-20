//! Setup runs with and without the Proxy.

use moor_core::domain::run::SyncMode;
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::secret::Secret;
use moor_core::{BoxId, BoxPath};

use super::{cfg, only, rig, run_setup, run_setup_with};
use crate::ops::common::box_exec::BoxProxy;
use crate::testing::driver::FakeDriver;

#[test]
fn without_a_proxy_setup_runs_in_workdir_with_exactly_the_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[("B", "2"), ("A", "1")]),
        SyncMode::Full,
    );
    let (_, spec, _) = only(r.driver.execs());
    assert_eq!(spec.cwd.as_ref(), Some(&r.workdir));
    assert_eq!(
        spec.env,
        vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned())
        ]
    );
}

#[test]
fn setup_gets_the_proxy_and_ca_variables_the_agent_gets() {
    let d = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let r = rig(
        d.path(),
        FakeDriver::new().peek_text("box-1", &environ, "HOME=/home/box\0"),
    );
    let proxy = BoxProxy {
        box_id: BoxId::new(1),
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret-value".to_vec()),
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            forge: false,
            box_env: vec![("A".to_owned(), "1".to_owned())],
            direct: Vec::new(),
        },
        origin: None,
        rule_env: Vec::new(),
    };
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[("A", "1")]),
        SyncMode::Full,
        Some(&proxy),
    );
    let (_, spec, _) = r
        .driver
        .execs()
        .into_iter()
        .find(|(_, s, _)| {
            s.argv
                .get(2)
                .is_some_and(|a| a.contains("export HTTPS_PROXY="))
        })
        .expect("setup exec with the proxy prelude");
    let prelude = spec.argv.get(2).unwrap();
    assert!(prelude.contains("export HTTPS_PROXY="), "{prelude}");
    assert!(prelude.contains("SSL_CERT_FILE"), "{prelude}");
    assert!(!prelude.contains("s3cret-value"), "{prelude}");
    assert_eq!(spec.env, vec![("A".to_owned(), "1".to_owned())]);
    assert_eq!(spec.cwd.as_ref(), Some(&r.workdir));
}

const RESERVED: (&str, &str) = ("NODE_OPTIONS", "--max-old-space-size=4096");
const CREDENTIAL_LIKE: (&str, &str) = ("STRIPE_TEST_KEY", "sk_test_x");

fn has_env(env: &[(String, String)], (k, v): (&str, &str)) -> bool {
    env.iter().any(|(ek, ev)| ek == k && ev == v)
}

#[test]
fn without_a_proxy_setup_gets_reserved_and_credential_like_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[RESERVED, CREDENTIAL_LIKE]),
        SyncMode::Full,
    );
    let (_, spec, _) = only(r.driver.execs());
    assert!(has_env(&spec.env, RESERVED), "{:?}", spec.env);
    assert!(has_env(&spec.env, CREDENTIAL_LIKE), "{:?}", spec.env);
}

fn proxy() -> BoxProxy {
    BoxProxy {
        box_id: BoxId::new(1),
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret-value".to_vec()),
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            forge: false,
            box_env: Vec::new(),
            direct: Vec::new(),
        },
        origin: None,
        rule_env: Vec::new(),
    }
}

#[test]
fn with_a_proxy_setup_still_gets_reserved_and_credential_like_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let r = rig(
        d.path(),
        FakeDriver::new().peek_text("box-1", &environ, "HOME=/home/box\0"),
    );
    // The agent's env: the Proxy plan drops both names.
    let mut proxy = proxy();
    proxy.addr.box_env = vec![("A".to_owned(), "1".to_owned())];
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[("A", "1"), RESERVED, CREDENTIAL_LIKE]),
        SyncMode::Full,
        Some(&proxy),
    );
    let (_, spec, _) = r
        .driver
        .execs()
        .into_iter()
        .find(|(_, s, _)| {
            s.argv
                .get(2)
                .is_some_and(|a| a.contains("export HTTPS_PROXY="))
        })
        .expect("setup exec with the proxy prelude");
    assert!(has_env(&spec.env, RESERVED), "{:?}", spec.env);
    assert!(has_env(&spec.env, CREDENTIAL_LIKE), "{:?}", spec.env);
}

/// Runs setup with the Proxy where the setup exec ends as `setup`, and checks
/// the exec right after it removes the staged credential dir.
fn assert_failed_setup_removes_the_credential_dir(setup: Result<ExecOutcome, DriverError>) {
    let d = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let r = rig(
        d.path(),
        FakeDriver::new()
            .peek_text("box-1", &environ, "HOME=/home/box\0")
            .exec_results([Ok(ExecOutcome::Exited { code: 0 }), setup]),
    );
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
        Some(&proxy()),
    );
    let argvs: Vec<Vec<String>> = r
        .driver
        .execs()
        .into_iter()
        .map(|(_, s, _)| s.argv)
        .collect();
    let mkdir = argvs
        .iter()
        .position(|a| a.starts_with(&["mkdir".to_owned(), "-m".to_owned(), "700".to_owned()]))
        .expect("mkdir of the credential dir");
    let dir = argvs.get(mkdir).and_then(|a| a.last()).cloned().unwrap();
    assert!(dir.contains("/.moor-cred-"), "{dir}");
    assert_eq!(
        argvs.get(mkdir + 2),
        Some(&vec!["rm".to_owned(), "-rf".to_owned(), dir]),
        "{argvs:?}"
    );
}

#[test]
fn an_interrupted_setup_removes_the_staged_credential_dir() {
    assert_failed_setup_removes_the_credential_dir(Ok(ExecOutcome::Interrupted));
}

#[test]
fn a_failed_setup_exec_removes_the_staged_credential_dir() {
    assert_failed_setup_removes_the_credential_dir(Err(DriverError::Quota("out".to_owned())));
}
