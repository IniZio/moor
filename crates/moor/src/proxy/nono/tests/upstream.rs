//! Fake HTTPS upstream on loopback that records what the proxy sends it.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::time::Duration;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Proxy};

/// What the upstream saw for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Request line, e.g. `GET /ok HTTP/1.1`.
    pub(crate) line: String,
    /// `Authorization` header value, if any.
    pub(crate) authorization: Option<String>,
    /// Whole request head, request line and every header.
    pub(crate) head: String,
}

/// A running fake upstream; stops its accept thread on drop.
pub(crate) struct Upstream {
    /// Loopback port it listens on.
    pub(crate) port: u16,
    /// Requests seen so far.
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Set to ask the accept loop to exit.
    stop: Arc<AtomicBool>,
    /// Accept thread.
    thread: Option<JoinHandle<()>>,
    /// TLS server settings shared with twins.
    config: Arc<ServerConfig>,
    /// Test CA certificate PEM file for `RouteConfig.tls_ca`.
    pub(crate) ca_path: PathBuf,
    /// Test CA certificate PEM.
    pub(crate) ca_pem: String,
}

impl Upstream {
    /// Requests recorded so far.
    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Test CA plus a leaf for 127.0.0.1 and localhost, as DER.
fn make_certs() -> (String, CertificateDer<'static>, PrivateKeyDer<'static>) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "moor test upstream CA");
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::default();
    leaf.subject_alt_names = vec![
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        SanType::DnsName("localhost".try_into().unwrap()),
    ];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();
    (
        ca_cert.pem(),
        leaf_cert.der().clone(),
        PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
    )
}

/// Reads one request head (up to the blank line) from `tls`.
fn read_head(tls: &mut impl Read) -> Option<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if tls.read(&mut byte).ok()? == 0 || buf.len() > 16 * 1024 {
            return None;
        }
        buf.push(byte[0]);
    }
    String::from_utf8(buf).ok()
}

/// Serves one connection: records the request, replies 401 for `/expired*`, else 200.
fn serve(stream: TcpStream, config: &Arc<ServerConfig>, seen: &Mutex<Vec<Seen>>) {
    let Ok(conn) = ServerConnection::new(Arc::clone(config)) else {
        return;
    };
    let mut tls = StreamOwned::new(conn, stream);
    if let Some(head) = read_head(&mut tls) {
        let mut lines = head.split("\r\n");
        let line = lines.next().unwrap_or_default().to_owned();
        let authorization = lines
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.trim().to_owned());
        let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Seen {
                line,
                authorization,
                head: head.clone(),
            });
        let (status, body) = if path.starts_with("/expired") {
            ("401 Unauthorized", "no")
        } else {
            ("200 OK", "ok")
        };
        let reply = format!(
            "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        if tls.write_all(reply.as_bytes()).is_ok() && tls.flush().is_ok() {
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    }
}

/// Starts the fake upstream; the CA PEM is written under `dir` as `upstream-ca.pem`.
pub(crate) fn start(dir: &Path) -> Upstream {
    let (ca_pem, leaf, key) = make_certs();
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![leaf], key)
            .unwrap();
    let ca_path = dir.join("upstream-ca.pem");
    std::fs::write(&ca_path, &ca_pem).unwrap();
    spawn(Arc::new(config), ca_path, ca_pem)
}

impl Upstream {
    /// A second upstream on another port with the same certificates.
    pub(crate) fn twin(&self) -> Self {
        spawn(
            Arc::clone(&self.config),
            self.ca_path.clone(),
            self.ca_pem.clone(),
        )
    }
}

/// Binds a loopback listener and serves it on a thread.
fn spawn(config: Arc<ServerConfig>, ca_path: PathBuf, ca_pem: String) -> Upstream {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (seen, stop, config) = (Arc::clone(&seen), Arc::clone(&stop), Arc::clone(&config));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let (config, seen) = (Arc::clone(&config), Arc::clone(&seen));
                std::thread::spawn(move || serve(stream, &config, &seen));
            }
        })
    };
    Upstream {
        port,
        seen,
        stop,
        thread: Some(thread),
        config,
        ca_path,
        ca_pem,
    }
}

/// A client that goes through the proxy on `proxy_port` as `moor:<nonce>` and trusts only `ca_pem`.
pub(crate) fn client(proxy_port: u16, nonce: &str, ca_pem: &str) -> Agent {
    let cert = Certificate::from_pem(ca_pem.as_bytes()).unwrap().to_owned();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::Specific(Arc::new(vec![cert])))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let proxy = Proxy::new(&format!("http://moor:{nonce}@127.0.0.1:{proxy_port}")).unwrap();
    let config = Agent::config_builder()
        .proxy(Some(proxy))
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build();
    Agent::new_with_config(config)
}
