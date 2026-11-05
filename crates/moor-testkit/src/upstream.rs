//! A local TLS upstream and a loop of requests that reach it through a Box's Proxy.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::PrivateKeyDer;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Proxy};

/// Longest a single request through the Proxy may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// An HTTPS server on loopback for `localhost`, signed by a CA of its own; it answers every
/// request `200` and records each `Authorization` it sees. Dropping it closes the port.
pub struct TlsUpstream {
    /// The loopback port.
    pub port: u16,
    /// The CA certificate PEM file, for the Proxy to trust.
    pub ca_path: PathBuf,
    /// Each request's `Authorization`, empty when it had none.
    seen: Arc<Mutex<Vec<String>>>,
    /// Tells the accept loop to stop.
    stop: Arc<AtomicBool>,
    /// The accept loop, which owns the listener.
    thread: Option<JoinHandle<()>>,
}

impl TlsUpstream {
    /// Starts the server and writes its CA to `dir/upstream-ca.pem`.
    pub fn start(dir: &Path) -> io::Result<Self> {
        let (ca_pem, config) = certs().map_err(io::Error::other)?;
        let ca_path = dir.join("upstream-ca.pem");
        std::fs::write(&ca_path, ca_pem)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (config, log, halt) = (Arc::new(config), Arc::clone(&seen), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if halt.load(Ordering::Acquire) {
                    return;
                }
                let (config, log) = (Arc::clone(&config), Arc::clone(&log));
                std::thread::spawn(move || answer(stream, config, &log));
            }
        });
        Ok(Self {
            port,
            ca_path,
            seen,
            stop,
            thread: Some(thread),
        })
    }

    /// Each request's `Authorization` so far.
    #[must_use]
    pub fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for TlsUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A CA PEM and a server config whose leaf, for `localhost` and `127.0.0.1`, that CA signed.
fn certs() -> Result<(String, ServerConfig), String> {
    let fail = |e: &dyn std::fmt::Display| e.to_string();
    let ca_key = KeyPair::generate().map_err(|e| fail(&e))?;
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca.distinguished_name
        .push(DnType::CommonName, "moor testkit upstream CA");
    let ca_cert = ca.self_signed(&ca_key).map_err(|e| fail(&e))?;
    let issuer = Issuer::new(ca, ca_key);
    let leaf_key = KeyPair::generate().map_err(|e| fail(&e))?;
    let mut leaf = CertificateParams::new(vec!["localhost".to_owned()]).map_err(|e| fail(&e))?;
    leaf.subject_alt_names
        .push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).map_err(|e| fail(&e))?;
    let key = PrivateKeyDer::try_from(leaf_key.serialize_der()).map_err(|e| fail(&e))?;
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| fail(&e))?
            .with_no_client_auth()
            .with_single_cert(vec![leaf_cert.der().clone()], key)
            .map_err(|e| fail(&e))?;
    Ok((ca_cert.pem(), config))
}

/// Reads one request head over TLS, records its `Authorization` and answers `200 ok`.
fn answer(stream: TcpStream, config: Arc<ServerConfig>, seen: &Mutex<Vec<String>>) {
    let Ok(conn) = ServerConnection::new(config) else {
        return;
    };
    let mut tls = StreamOwned::new(conn, stream);
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 << 10 || !matches!(tls.read(&mut byte), Ok(1)) {
            return;
        }
        head.extend_from_slice(&byte);
    }
    let head = String::from_utf8_lossy(&head);
    let auth = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.trim().to_owned());
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(auth.unwrap_or_default());
    let reply = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";
    if tls.write_all(reply.as_bytes()).is_ok() {
        tls.conn.send_close_notify();
        let _ = tls.flush();
    }
}

/// A client that reaches the web as a Box does: through the Proxy at `127.0.0.1:box_port`,
/// trusting only the Box CA `box_ca_pem`.
pub fn box_client(box_port: u16, box_ca_pem: &str) -> Result<Agent, String> {
    let cert = Certificate::from_pem(box_ca_pem.as_bytes())
        .map_err(|e| e.to_string())?
        .to_owned();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::Specific(Arc::new(vec![cert])))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let proxy = Proxy::new(&format!("http://127.0.0.1:{box_port}")).map_err(|e| e.to_string())?;
    let config = Agent::config_builder()
        .proxy(Some(proxy))
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build();
    Ok(Agent::new_with_config(config))
}

/// What a [`Traffic`] loop saw.
#[derive(Debug, Clone, Default)]
pub struct Tally {
    /// Requests that succeeded.
    pub ok: usize,
    /// Why each failed request failed, in order.
    pub failed: Vec<String>,
}

/// A thread sending one request after another until stopped.
pub struct Traffic {
    /// What it saw so far.
    tally: Arc<Mutex<Tally>>,
    /// Tells the thread to stop.
    stop: Arc<AtomicBool>,
    /// The loop.
    thread: Option<JoinHandle<()>>,
}

impl Traffic {
    /// Runs `request` every `pause` until [`Self::stop`]; `Err` counts as a failed request.
    pub fn start(
        pause: Duration,
        mut request: impl FnMut() -> Result<(), String> + Send + 'static,
    ) -> Self {
        let tally = Arc::new(Mutex::new(Tally::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (t, s) = (Arc::clone(&tally), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Acquire) {
                let got = request();
                let mut t = t.lock().unwrap_or_else(PoisonError::into_inner);
                match got {
                    Ok(()) => t.ok += 1,
                    Err(e) => t.failed.push(e),
                }
                drop(t);
                std::thread::sleep(pause);
            }
        });
        Self {
            tally,
            stop,
            thread: Some(thread),
        }
    }

    /// What the loop saw so far.
    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stops the loop after its request in flight; returns what it saw.
    #[must_use]
    pub fn stop(mut self) -> Tally {
        self.halt();
        self.tally()
    }

    /// Asks the loop to stop and waits for it.
    fn halt(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Traffic {
    fn drop(&mut self) {
        self.halt();
    }
}
