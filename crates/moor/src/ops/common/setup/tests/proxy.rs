//! Setup runs with and without the Proxy.

use moor_core::domain::run::SyncMode;
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::secret::Secret;

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
    let r = rig(d.path(), FakeDriver::new());
    let proxy = BoxProxy {
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret-value".to_vec()),
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            box_env: vec![("A".to_owned(), "1".to_owned())],
        },
        origin: None,
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
    let (_, spec, _) = r.driver.execs().remove(0);
    let prelude = spec.argv.get(2).unwrap();
    assert!(prelude.contains("export HTTPS_PROXY="), "{prelude}");
    assert!(prelude.contains("SSL_CERT_FILE"), "{prelude}");
    assert!(!prelude.contains("s3cret-value"), "{prelude}");
    assert_eq!(spec.env, vec![("A".to_owned(), "1".to_owned())]);
    assert_eq!(spec.cwd.as_ref(), Some(&r.workdir));
}
