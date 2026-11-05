//! A ledger address: a socket path, or a TLS address that only a CA bundle may check.

use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, KeyPair};

use super::super::Endpoint;

/// A PEM file holding one self-signed certificate.
fn ca(dir: &Path) -> PathBuf {
    let cert = CertificateParams::default()
        .self_signed(&KeyPair::generate().unwrap())
        .unwrap();
    let path = dir.join("ca.pem");
    std::fs::write(&path, cert.pem()).unwrap();
    path
}

#[test]
fn a_path_is_the_ledger_socket() {
    let at = Endpoint::parse("/state/ledger/proxy.sock", None).unwrap();
    assert!(matches!(at, Endpoint::Unix(p) if p == Path::new("/state/ledger/proxy.sock")));
}

#[test]
fn a_tls_address_is_checked_against_its_ca_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca(dir.path());
    for addr in [
        "tls://ledger.moor.svc:7443",
        "tls://127.0.0.1:7443",
        "tls://[::1]:7443",
    ] {
        let at = Endpoint::parse(addr, Some(&ca)).unwrap();
        assert!(matches!(at, Endpoint::Tls(_)), "{addr}");
    }
}

#[test]
fn a_tls_address_without_a_ca_bundle_is_refused() {
    let err = Endpoint::parse("tls://ledger.moor.svc:7443", None).unwrap_err();
    assert!(err.contains("CA bundle"), "{err}");
}

#[test]
fn a_ca_bundle_with_a_socket_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let err = Endpoint::parse("/state/ledger/proxy.sock", Some(&ca(dir.path()))).unwrap_err();
    assert!(err.contains("tls://"), "{err}");
}

#[test]
fn a_bad_tls_address_or_an_empty_bundle_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca(dir.path());
    for bad in [
        "tls://ledger.moor.svc",
        "tls://ledger.moor.svc:port",
        "tls://:7443",
    ] {
        assert!(Endpoint::parse(bad, Some(&ca)).is_err(), "{bad}");
    }
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    let err = Endpoint::parse("tls://ledger.moor.svc:7443", Some(&empty)).unwrap_err();
    assert!(err.contains("no certificate"), "{err}");
}
