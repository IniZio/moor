//! A ledger address: a socket path, or a TLS address dialled with a CA bundle and a client
//! certificate.

use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, KeyPair};

use super::super::{Endpoint, TlsFiles};

/// A self-signed certificate as `ca.pem`, doubling as the client certificate `client.pem` with
/// its key `client.key`.
fn files(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::default().self_signed(&key).unwrap();
    let paths = (
        dir.join("ca.pem"),
        dir.join("client.pem"),
        dir.join("client.key"),
    );
    std::fs::write(&paths.0, cert.pem()).unwrap();
    std::fs::write(&paths.1, cert.pem()).unwrap();
    std::fs::write(&paths.2, key.serialize_pem()).unwrap();
    paths
}

fn tls(paths: &(PathBuf, PathBuf, PathBuf)) -> TlsFiles<'_> {
    TlsFiles {
        ca: &paths.0,
        cert: &paths.1,
        key: &paths.2,
    }
}

#[test]
fn a_path_is_the_ledger_socket() {
    let at = Endpoint::parse("/state/ledger/proxy.sock", None).unwrap();
    assert!(matches!(at, Endpoint::Unix(p) if p == Path::new("/state/ledger/proxy.sock")));
}

#[test]
fn a_tls_address_is_dialled_with_its_ca_bundle_and_client_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let paths = files(dir.path());
    for addr in [
        "tls://ledger.moor.svc:7443",
        "tls://127.0.0.1:7443",
        "tls://[::1]:7443",
    ] {
        let at = Endpoint::parse(addr, Some(tls(&paths))).unwrap();
        assert!(matches!(at, Endpoint::Tls(_)), "{addr}");
    }
}

#[test]
fn a_tls_address_without_its_files_is_refused() {
    let err = Endpoint::parse("tls://ledger.moor.svc:7443", None).unwrap_err();
    assert!(err.contains("CA bundle"), "{err}");
    assert!(err.contains("client certificate"), "{err}");
}

#[test]
fn tls_files_with_a_socket_path_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let paths = files(dir.path());
    let err = Endpoint::parse("/state/ledger/proxy.sock", Some(tls(&paths))).unwrap_err();
    assert!(err.contains("tls://"), "{err}");
}

#[test]
fn a_bad_tls_address_an_empty_bundle_or_a_missing_client_key_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let paths = files(dir.path());
    for bad in [
        "tls://ledger.moor.svc",
        "tls://ledger.moor.svc:port",
        "tls://:7443",
    ] {
        assert!(Endpoint::parse(bad, Some(tls(&paths))).is_err(), "{bad}");
    }
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    let no_ca = TlsFiles {
        ca: &empty,
        ..tls(&paths)
    };
    let err = Endpoint::parse("tls://ledger.moor.svc:7443", Some(no_ca)).unwrap_err();
    assert!(err.contains("no certificate"), "{err}");
    let missing = dir.path().join("missing.key");
    let no_key = TlsFiles {
        key: &missing,
        ..tls(&paths)
    };
    let err = Endpoint::parse("tls://ledger.moor.svc:7443", Some(no_key)).unwrap_err();
    assert!(err.contains("missing.key"), "{err}");
}
