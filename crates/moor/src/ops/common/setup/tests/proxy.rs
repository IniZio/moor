//! Setup runs with and without the Proxy.

use moor_core::seam::agent_profile::ProxyAddr;

use moor_core::domain::box_state::SetupOutcome;

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
    let r = rig(d.path(), FakeDriver::new());
    let proxy = BoxProxy {
        addr: ProxyAddr {
            box_port: 4000,
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            forge: false,
            box_env: vec![("A".to_owned(), "1".to_owned())],
            egress_port: 4001,
            hosts: Vec::new(),
        },
        rule_env: Vec::new(),
    };
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[("A", "1")]),
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
    );
    let (_, spec, _) = only(r.driver.execs());
    assert!(has_env(&spec.env, RESERVED), "{:?}", spec.env);
    assert!(has_env(&spec.env, CREDENTIAL_LIKE), "{:?}", spec.env);
}

fn proxy() -> BoxProxy {
    BoxProxy {
        addr: ProxyAddr {
            box_port: 4000,
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            forge: false,
            box_env: Vec::new(),
            egress_port: 4001,
            hosts: Vec::new(),
        },
        rule_env: Vec::new(),
    }
}

#[test]
fn with_a_proxy_setup_still_gets_reserved_and_credential_like_repo_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
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

#[test]
fn an_egress_proxy_that_cannot_start_is_the_setup_failure_and_no_line_is_blamed() {
    let d = tempfile::tempdir().unwrap();
    let stop = "the egress proxy could not start: apt-get could not install tinyproxy-bin: E: down";
    let driver = FakeDriver::new()
        .exec_results(super::exits(&[1]))
        .exec_stdout([format!("moor: {stop}\n")]);
    let r = rig(d.path(), driver);
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["first", "second"], &[]),
    );
    assert_eq!(
        got.outcome(),
        SetupOutcome::Failed {
            line: stop.to_owned()
        }
    );
    let warning = only(r.cx.take_warnings());
    assert_eq!(warning.code, "setup_failed");
    assert!(warning.message.contains(stop), "{}", warning.message);
    assert!(!warning.message.contains("first"), "{}", warning.message);
}
