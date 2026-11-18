//! The shared egress Proxy's command line.

use std::ffi::OsString;

use moor_core::config::Served;

use super::{keyed, main, Args};
use crate::env::Env;
use crate::proxy::egress::ServedRoute;

fn words(extra: &[&str]) -> Vec<OsString> {
    let mut all = vec![
        "--state-dir",
        "/nonexistent",
        "--listen",
        "127.0.0.1:0",
        "--ledger",
        "tls://ledger.moor.svc:7443",
    ];
    all.extend_from_slice(extra);
    all.into_iter().map(OsString::from).collect()
}

#[test]
fn a_partial_set_of_ledger_tls_files_is_refused() {
    for extra in [
        &["--ledger-ca", "/ca.pem"][..],
        &["--ledger-ca", "/ca.pem", "--ledger-cert", "/c.pem"],
        &["--ledger-cert", "/c.pem", "--ledger-key", "/c.key"],
        &["--ledger-key", "/c.key"],
    ] {
        let err = main(&words(extra)).expect_err("started with a partial set");
        assert!(err.contains("go together"), "{extra:?}: {err}");
    }
}

#[test]
fn a_tls_ledger_without_its_files_is_refused() {
    let err = main(&words(&[])).expect_err("started a tls:// ledger with no files");
    assert!(err.contains("client certificate"), "{err}");
}

#[test]
fn each_served_host_has_its_flag_and_a_public_upstream_is_refused() {
    let args = Args::parse(&words(&[
        "--machine-service",
        "machine.moor.internal=moor-machine.moor.svc:7700",
        "--moor-service",
        "moor.moor.internal=moor-gate-boxes.moor.svc:8444",
    ]))
    .unwrap();
    let served: Vec<_> = args
        .served
        .iter()
        .map(|r| (r.served, r.host.as_str(), r.upstream.as_str()))
        .collect();
    assert_eq!(
        served,
        [
            (
                Served::Machine,
                "machine.moor.internal",
                "moor-machine.moor.svc:7700"
            ),
            (
                Served::Moor,
                "moor.moor.internal",
                "moor-gate-boxes.moor.svc:8444"
            ),
        ]
    );
    let err = Args::parse(&words(&[
        "--moor-service",
        "moor.moor.internal=8.8.8.8:8444",
    ]))
    .expect_err("took a public upstream");
    assert!(err.contains("--moor-service"), "{err}");
}

fn route(served: Served) -> ServedRoute {
    ServedRoute::parse(served, "h.moor.internal=127.0.0.1:1").unwrap()
}

fn key_of(served: Served, env: &[(&str, &str)]) -> Result<Option<String>, String> {
    keyed(route(served), &Env::from_pairs(env.iter().copied()))
        .map(|r| r.key.map(|k| k.to_string()))
}

#[test]
fn moor_host_gets_moor_service_key_and_machine_host_never_does() {
    let env = [
        ("MOOR_SERVICE_KEY", "mk"),
        ("MOOR_MACHINE_SERVICE_KEY", "mc"),
    ];
    assert_eq!(key_of(Served::Moor, &env), Ok(Some("mk".to_owned())));
    assert_eq!(key_of(Served::Machine, &env), Ok(Some("mc".to_owned())));
    assert_eq!(
        key_of(Served::Machine, &[("MOOR_SERVICE_KEY", "mk")]),
        Ok(None)
    );
}

#[test]
fn no_moor_service_key_means_none_is_stamped() {
    assert_eq!(key_of(Served::Moor, &[]), Ok(None));
}

#[test]
fn an_empty_moor_service_key_refuses_to_start() {
    let err = key_of(Served::Moor, &[("MOOR_SERVICE_KEY", "")]).expect_err("started");
    assert!(err.contains("MOOR_SERVICE_KEY"), "{err}");
}
