//! The runner's TLS dial presents its client certificate to a listener that demands one
//! signed by the ledger CA.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test setup")]

use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};

use super::{client_config, line, trusting, Tls, TlsPki};
use crate::{Attempt, ClientError};

/// A CA in `dir` as `<name>-ca.pem`, and an issuer for it.
fn ca(dir: &Path, name: &str) -> Issuer<'static, KeyPair> {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.join(format!("{name}-ca.pem")), cert.pem()).unwrap();
    Issuer::new(params, key)
}

/// A certificate `issuer` signed for `purpose`, as `<name>.pem` with its key as `<name>.key`.
fn leaf(dir: &Path, name: &str, issuer: &Issuer<'_, KeyPair>, purpose: ExtendedKeyUsagePurpose) {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![SanType::IpAddress([127, 0, 0, 1].into())];
    params.distinguished_name.push(DnType::CommonName, name);
    params.extended_key_usages = vec![purpose];
    let cert = params.signed_by(&key, issuer).unwrap();
    std::fs::write(dir.join(format!("{name}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{name}.key")), key.serialize_pem()).unwrap();
}

/// The ledger CA with the controller's and the runner's certificates, and a stranger CA with
/// a runner certificate of its own.
fn pki(dir: &Path) {
    let ledger = ca(dir, "ledger");
    leaf(
        dir,
        "controller",
        &ledger,
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    leaf(dir, "runner", &ledger, ExtendedKeyUsagePurpose::ClientAuth);
    let stranger = ca(dir, "stranger");
    leaf(
        dir,
        "stranger",
        &stranger,
        ExtendedKeyUsagePurpose::ClientAuth,
    );
}

/// The runner's files, presenting certificate `cert`.
fn files(dir: &Path, cert: &str) -> TlsPki {
    TlsPki {
        ca: dir.join("ledger-ca.pem"),
        cert: dir.join(format!("{cert}.pem")),
        key: dir.join(format!("{cert}.key")),
    }
}

/// A listener demanding a client certificate the ledger CA signed. It takes one connection,
/// answers its hello, and returns how the handshake went.
fn listener(dir: &Path) -> (u16, JoinHandle<Result<(), String>>) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(dir.join("ledger-ca.pem")).unwrap())
        .unwrap();
    let verifier =
        WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()
            .unwrap();
    let chain = vec![CertificateDer::from_pem_file(dir.join("controller.pem")).unwrap()];
    let key = PrivateKeyDer::from_pem_file(dir.join("controller.key")).unwrap();
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .unwrap();
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = tcp.local_addr().unwrap().port();
    let served = std::thread::spawn(move || {
        let (sock, _) = tcp.accept().unwrap();
        let conn = ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = StreamOwned::new(conn, sock);
        let answered = line(&mut tls).and_then(|_| writeln!(tls, r#"{{"Ok":null}}"#));
        // Reading to EOF before closing keeps a reset from overtaking the alert.
        tls.sock.shutdown(Shutdown::Write).ok();
        std::io::copy(
            &mut tls.sock.try_clone().unwrap().take(1 << 16),
            &mut std::io::sink(),
        )
        .ok();
        answered.map_err(|e| e.to_string())
    });
    (port, served)
}

/// A runner dialing `port` with `config`.
fn runner(port: u16, config: rustls::ClientConfig) -> Tls {
    Tls {
        host: "127.0.0.1".to_owned(),
        port,
        config: Arc::new(config),
        hello: Arc::from(r#"{"runner":{"v":1,"key":"service-key"}}"#),
    }
}

fn dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_owned();
    pki(&path);
    (tmp, path)
}

#[test]
fn a_runner_with_the_ledger_cas_client_certificate_completes_its_hello() {
    let (_tmp, dir) = dir();
    let (port, served) = listener(&dir);
    let config = client_config(&files(&dir, "runner")).expect("config");
    assert!(matches!(runner(port, config).connect(), Attempt::Done(_)));
    assert_eq!(served.join().unwrap(), Ok(()));
}

#[test]
fn a_runner_without_a_client_certificate_is_refused_at_the_handshake() {
    let (_tmp, dir) = dir();
    let (port, served) = listener(&dir);
    let config = trusting(&dir.join("ledger-ca.pem"))
        .unwrap()
        .with_no_client_auth();
    let dialed = runner(port, config).connect();
    let why = served
        .join()
        .unwrap()
        .expect_err("served without a certificate");
    assert!(why.contains("certificate"), "{why}");
    assert!(
        !matches!(dialed, Attempt::Done(_)),
        "the hello was answered"
    );
}

#[test]
fn a_runner_whose_certificate_another_ca_signed_is_refused_at_the_handshake() {
    let (_tmp, dir) = dir();
    let (port, served) = listener(&dir);
    let config = client_config(&files(&dir, "stranger")).expect("config");
    let dialed = runner(port, config).connect();
    let why = served
        .join()
        .unwrap()
        .expect_err("served a stranger's certificate");
    assert!(why.contains("certificate"), "{why}");
    assert!(
        matches!(
            &dialed,
            Attempt::Failed(ClientError::Unauthorized(_)) | Attempt::Unrun(_)
        ),
        "{dialed:?}"
    );
}

#[test]
fn a_client_key_that_does_not_fit_its_certificate_is_refused_before_any_dial() {
    let (_tmp, dir) = dir();
    let mut mismatched = files(&dir, "runner");
    mismatched.key = dir.join("stranger.key");
    let why = client_config(&mismatched).expect_err("a mismatched key was taken");
    assert!(why.contains("does not fit"), "{why}");
}
