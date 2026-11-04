use std::io::{Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use postgres::config::SslMode as Negotiate;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType};
use rustls::pki_types::PrivateKeyDer;
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::Target;

#[test]
fn shown_url_hides_the_password_and_the_query() {
    let target =
        Target::parse("postgres://moor:s3cret@db:5432/moor?sslmode=disable&password=other")
            .unwrap();
    assert_eq!(target.shown(), "postgres://moor:***@db:5432/moor");
    assert_eq!(format!("{target:?}"), "postgres://moor:***@db:5432/moor");
    assert_eq!(
        target.url(),
        "postgres://moor:s3cret@db:5432/moor?sslmode=disable&password=other"
    );
}

#[test]
fn a_refused_url_never_shows_its_password() {
    for url in [
        "mysql://moor:s3cret@db/moor",
        "postgres://moor:s3cret@db/moor?sslmode=allow",
        "postgres://moor:s3cret@db/moor?connect_timeout=soon",
        "postgres://moor:s3cret@db/moor?sslmode=verify-full&sslrootcert=/nonexistent/ca.pem",
        "moor:s3cret@db",
    ] {
        let err = Target::parse(url).unwrap_err();
        assert!(!err.contains("s3cret"), "{url}: {err}");
    }
}

#[test]
fn sslmode_sets_whether_tls_is_asked_for() {
    let negotiated = |query: &str| {
        Target::parse(&format!("postgres://moor@db/moor{query}"))
            .unwrap()
            .config()
            .get_ssl_mode()
    };
    assert_eq!(negotiated(""), Negotiate::Prefer);
    assert_eq!(negotiated("?sslmode=disable"), Negotiate::Disable);
    assert_eq!(negotiated("?sslmode=prefer"), Negotiate::Prefer);
    assert_eq!(negotiated("?sslmode=require"), Negotiate::Require);
    assert_eq!(negotiated("?sslmode=verify-full"), Negotiate::Require);
}

#[test]
fn other_query_fields_reach_the_connection_settings() {
    let target = Target::parse(
        "postgres://moor@db/moor?application_name=moor-test&sslmode=disable&options=-c%20search_path%3Dx&connect_timeout=3",
    )
    .unwrap();
    let config = target.config();
    assert_eq!(config.get_application_name(), Some("moor-test"));
    assert_eq!(config.get_options(), Some("-c search_path=x"));
    assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(3)));
    assert_eq!(
        target.read_only().config().get_options(),
        Some("-c search_path=x -c default_transaction_read_only=on")
    );
}

#[test]
fn a_url_without_a_connect_timeout_waits_ten_seconds() {
    let target = Target::parse("postgresql://moor@db/moor").unwrap();
    assert_eq!(
        target.config().get_connect_timeout(),
        Some(&Duration::from_secs(10))
    );
}

/// A CA, and a server certificate it signed for `name`, with the server's key.
struct Pki {
    /// The CA's certificate, PEM.
    ca_pem: String,
    /// The server's certificate.
    cert: rustls::pki_types::CertificateDer<'static>,
    /// The server's key, DER.
    key: Vec<u8>,
}

fn pki(name: SanType) -> Pki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca, ca_key);
    let key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::default();
    leaf.subject_alt_names = vec![name];
    let cert = leaf.signed_by(&key, &issuer).unwrap();
    Pki {
        ca_pem: ca_cert.pem(),
        cert: cert.der().clone(),
        key: key.serialize_der(),
    }
}

/// Serve one Postgres TLS handshake on 127.0.0.1 with `pki`'s certificate; its port.
fn tls_server(pki: &Pki) -> u16 {
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![pki.cert.clone()],
                PrivateKeyDer::try_from(pki.key.clone()).unwrap(),
            )
            .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut ssl_request = [0u8; 8];
        if sock.read_exact(&mut ssl_request).is_err() || sock.write_all(b"S").is_err() {
            return;
        }
        let conn = ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = StreamOwned::new(conn, sock);
        let mut startup = [0u8; 8];
        drop(tls.read(&mut startup));
    });
    port
}

/// Whether a client connecting with `query` completes the TLS handshake with a server holding a
/// certificate for `name` that `pki`'s CA signed. `trusted` writes that CA as `sslrootcert`,
/// else another CA.
fn handshakes(query: &str, name: SanType, trusted: bool) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let server = pki(name);
    let root = dir.path().join("root.pem");
    let pem = if trusted {
        server.ca_pem.clone()
    } else {
        pki(SanType::DnsName("other".try_into().unwrap())).ca_pem
    };
    std::fs::write(&root, pem).unwrap();
    let port = tls_server(&server);
    let query = query.replace("ROOT", &root.display().to_string());
    let url = format!("postgres://moor:pw@127.0.0.1:{port}/moor?connect_timeout=5&{query}");
    let err = Target::parse(&url).unwrap().connect().err().unwrap();
    !err.to_string().contains("TLS handshake")
}

fn ip() -> SanType {
    SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

fn other_host() -> SanType {
    SanType::DnsName("db.example".try_into().unwrap())
}

#[test]
fn verify_full_needs_a_trusted_certificate_naming_the_host() {
    let full = "sslmode=verify-full&sslrootcert=ROOT";
    assert!(handshakes(full, ip(), true));
    assert!(!handshakes(full, other_host(), true));
    assert!(!handshakes(full, ip(), false));
}

#[test]
fn verify_ca_needs_a_trusted_certificate_for_any_host() {
    let ca = "sslmode=verify-ca&sslrootcert=ROOT";
    assert!(handshakes(ca, other_host(), true));
    assert!(!handshakes(ca, other_host(), false));
}

#[test]
fn require_encrypts_without_checking_unless_given_roots() {
    assert!(handshakes("sslmode=require", other_host(), false));
    assert!(!handshakes(
        "sslmode=require&sslrootcert=ROOT",
        other_host(),
        false
    ));
}
