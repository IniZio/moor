//! Tests for the upstream TLS client against a loopback server.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::PrivateKeyDer;
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;

/// Loopback TLS server with a leaf for `api.github.com`, answering one connection.
struct Server {
    /// Address it listens on.
    addr: SocketAddr,
    /// PEM file holding the test CA.
    ca: PathBuf,
    /// Keeps the CA file alive.
    _dir: tempfile::TempDir,
    /// Serving thread; yields the bytes the client sent.
    thread: JoinHandle<Vec<u8>>,
}

/// Starts a server that reads 4 bytes, echoes them upper-cased, then closes.
fn start() -> Server {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);
    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::default();
    leaf.subject_alt_names = vec![SanType::DnsName("api.github.com".try_into().unwrap())];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf_cert.der().clone()],
                PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
            )
            .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, ca_cert.pem()).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Arc::new(config);
    let thread = std::thread::spawn(move || {
        let mut got = [0u8; 4];
        let Ok((tcp, _)) = listener.accept() else {
            return Vec::new();
        };
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
        if tls.read_exact(&mut got).is_ok() {
            let _ = tls.write_all(&got.to_ascii_uppercase());
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
        got.to_vec()
    });
    Server {
        addr,
        ca,
        _dir: dir,
        thread,
    }
}

#[test]
fn handshake_with_extra_root_round_trips_bytes() {
    let server = start();
    let up = Upstream::new("api.github.com", 443, Some(server.addr), Some(&server.ca)).unwrap();
    assert_eq!(up.host(), "api.github.com");
    let mut tls = up.connect().unwrap();
    assert_eq!(tls.conn.alpn_protocol(), Some(&b"http/1.1"[..]));
    tls.write_all(b"ping").unwrap();
    let mut reply = [0u8; 4];
    tls.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"PING");
    assert_eq!(server.thread.join().unwrap(), b"ping");
}

#[test]
fn unknown_ca_is_rejected() {
    let server = start();
    let up = Upstream::new("api.github.com", 443, Some(server.addr), None).unwrap();
    let err = up.connect().unwrap_err();
    assert!(matches!(err, UpstreamError::Handshake(_)), "{err}");
    let _ = server.thread.join();
}

#[test]
fn wrong_host_name_is_rejected() {
    let server = start();
    let up = Upstream::new("example.com", 443, Some(server.addr), Some(&server.ca)).unwrap();
    let err = up.connect().unwrap_err();
    assert!(matches!(err, UpstreamError::Handshake(_)), "{err}");
    let _ = server.thread.join();
}

#[test]
fn missing_extra_root_file_is_an_error() {
    let err = Upstream::new(
        "api.github.com",
        443,
        None,
        Some(std::path::Path::new("/nope.pem")),
    )
    .unwrap_err();
    assert!(matches!(err, UpstreamError::Roots { .. }), "{err}");
}

#[test]
fn debug_omits_config() {
    let up = Upstream::new("api.github.com", 443, None, None).unwrap();
    let text = format!("{up:?}");
    assert!(text.contains("api.github.com"));
    assert!(!text.contains("ClientConfig"));
}

#[test]
fn guard_refuses_loopback_name() {
    let up = Upstream::new("localhost", 443, None, None).unwrap();
    let err = up.connect().unwrap_err();
    assert!(
        matches!(err, UpstreamError::Guard(GuardError::NotPublic { .. })),
        "{err}"
    );
}
