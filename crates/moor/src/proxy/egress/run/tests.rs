//! The shared egress Proxy's command line.

use std::ffi::OsString;

use super::main;

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
