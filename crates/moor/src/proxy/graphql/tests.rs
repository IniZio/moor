//! Loopback tests of the GraphQL filter against a fake upstream.
//!
//! The client behaves like nono: it opens a tunnel with `CONNECT`, does TLS
//! under the Task CA and sends requests that already carry the credential.

mod egress;
mod framing;
mod scope;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::proxy::guard::Guard;
use moor_core::graphql_scope::RepoRef;
use nono_proxy::tls_intercept::EphemeralCa;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use serde_json::{json, Value};

use super::upstream::Upstream;
use super::{FilterSpec, GraphqlFilter, Refusal};

/// The credential nono has already injected; tests never print it.
const TOKEN: &str = "gho_SECRETTOKEN";
/// Rule name the filter reports in refusals.
const RULE: &str = "github";
/// Answer of the fake upstream to anything that is not a lookup.
const OK_BODY: &str = r#"{"data":{"ok":true}}"#;

/// A parsed HTTP message together with the exact bytes it came from.
#[derive(Debug, Clone)]
struct Wire {
    /// Head and body exactly as read.
    raw: Vec<u8>,
    /// Head including the blank line.
    head: String,
    /// Body with any chunked framing removed.
    body: Vec<u8>,
}

impl Wire {
    /// Value of header `name`, matched case-insensitively.
    fn header(&self, name: &str) -> Option<&str> {
        self.head
            .split("\r\n")
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }

    /// Status code of a response.
    fn status(&self) -> u16 {
        self.head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// Whether this is the filter's node lookup.
    fn is_lookup(&self) -> bool {
        String::from_utf8_lossy(&self.body).contains("$ids:[ID!]!")
    }

    /// The `message` of a JSON error body, if any.
    fn message(&self) -> Option<String> {
        let v: Value = serde_json::from_slice(&self.body).ok()?;
        v.get("message")?.as_str().map(str::to_owned)
    }
}

/// Reads one message; `eof_body` lets a body without framing run to end of stream.
fn read_message(r: &mut impl BufRead, eof_body: bool) -> Option<Wire> {
    let mut raw = Vec::new();
    while !raw.ends_with(b"\r\n\r\n") {
        if r.read_until(b'\n', &mut raw).ok()? == 0 {
            return None;
        }
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let mut wire = Wire {
        raw,
        head,
        body: Vec::new(),
    };
    let chunked = wire
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        read_chunks(r, &mut wire)?;
    } else if let Some(len) = wire.header("content-length") {
        let mut body = vec![0u8; len.parse().ok()?];
        r.read_exact(&mut body).ok()?;
        wire.raw.extend_from_slice(&body);
        wire.body = body;
    } else if eof_body {
        let mut body = Vec::new();
        r.read_to_end(&mut body).ok()?;
        wire.raw.extend_from_slice(&body);
        wire.body = body;
    }
    Some(wire)
}

/// Appends a chunked body, framing included, to `wire.raw` and the payload to `wire.body`.
fn read_chunks(r: &mut impl BufRead, wire: &mut Wire) -> Option<()> {
    loop {
        let start = wire.raw.len();
        r.read_until(b'\n', &mut wire.raw).ok()?;
        let line = String::from_utf8_lossy(wire.raw.get(start..)?).into_owned();
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        if size == 0 {
            loop {
                let start = wire.raw.len();
                r.read_until(b'\n', &mut wire.raw).ok()?;
                if wire.raw.get(start..)? == b"\r\n" {
                    return Some(());
                }
            }
        }
        let mut data = vec![0u8; size + 2];
        r.read_exact(&mut data).ok()?;
        wire.raw.extend_from_slice(&data);
        wire.body.extend_from_slice(data.get(..size)?);
    }
}

/// Builds the reply of the fake upstream to one request.
type Responder = Arc<dyn Fn(&Wire) -> Vec<u8> + Send + Sync>;

/// Fake `api.github.com`: a TLS server that records every request it gets.
struct Up {
    /// Where it listens.
    addr: SocketAddr,
    /// PEM file with the CA that signed its leaf.
    ca: PathBuf,
    /// Keeps the CA file alive.
    _dir: tempfile::TempDir,
    /// Every request received, in order.
    seen: Arc<Mutex<Vec<Wire>>>,
    /// Tells the accept loop to end.
    stop: Arc<AtomicBool>,
}

impl Drop for Up {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Server TLS config with an `api.github.com` leaf, and the PEM of its CA.
fn upstream_tls() -> (Arc<ServerConfig>, String) {
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
    (Arc::new(config), ca_cert.pem())
}

/// Serves one upstream connection until the peer closes or asks to close.
fn serve_upstream(tcp: TcpStream, config: Arc<ServerConfig>, up: &Mutex<Vec<Wire>>, f: &Responder) {
    tcp.set_nonblocking(false).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
    let mut r = BufReader::new(tls);
    while let Some(req) = read_message(&mut r, false) {
        let reply = f(&req);
        let closing = req
            .header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        up.lock().unwrap_or_else(PoisonError::into_inner).push(req);
        let tls = r.get_mut();
        if tls.write_all(&reply).and_then(|()| tls.flush()).is_err() || closing {
            break;
        }
    }
    let tls = r.get_mut();
    tls.conn.send_close_notify();
    let _ = tls.flush();
}

impl Up {
    /// Starts the server; `f` builds every reply from the request.
    fn start(f: Responder) -> Self {
        let (config, ca_pem) = upstream_tls();
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, ca_pem).unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                if let Ok((tcp, _)) = listener.accept() {
                    let (config, seen, f) = (Arc::clone(&config), Arc::clone(&seen2), f.clone());
                    std::thread::spawn(move || serve_upstream(tcp, config, &seen, &f));
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        Self {
            addr,
            ca,
            _dir: dir,
            seen,
            stop,
        }
    }
}

/// The filter under test with its fake upstream.
struct Rig {
    /// The filter.
    filter: GraphqlFilter,
    /// The fake upstream behind it.
    up: Up,
    /// PEM of the Task CA the client trusts.
    ca_pem: String,
}

/// The repo every test scopes the Task to.
fn task() -> RepoRef {
    RepoRef::new("IniZio", "moor")
}

/// Starts a filter whose upstream answers with `f`.
fn rig(f: Responder) -> Rig {
    rig_with(f, Guard::default())
}

/// Starts a filter that vets other destinations with `guard`.
fn rig_with(f: Responder, guard: Guard) -> Rig {
    let ca = EphemeralCa::generate().unwrap();
    let ca_pem = ca.cert_pem().to_owned();
    let up = Up::start(f);
    let upstream = Upstream::new("api.github.com", 443, Some(up.addr), Some(&up.ca)).unwrap();
    let filter = GraphqlFilter::start(FilterSpec {
        ca,
        repo: Some(task()),
        host: "api.github.com".to_owned(),
        port: 443,
        rule: RULE.to_owned(),
        upstream,
        guard,
    })
    .unwrap();
    Rig { filter, up, ca_pem }
}

impl Rig {
    /// Requests the upstream received so far.
    fn requests(&self) -> Vec<Wire> {
        self.up
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many of those were node lookups.
    fn lookups(&self) -> usize {
        self.requests().iter().filter(|w| w.is_lookup()).count()
    }

    /// Waits for at least `want` refusals, then returns every refusal recorded.
    fn refusals(&self, want: usize) -> Vec<Refusal> {
        let mut got = Vec::new();
        for _ in 0..100 {
            got.extend(self.filter.drain_refusals());
            if got.len() >= want {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(Duration::from_millis(150));
        got.extend(self.filter.drain_refusals());
        got
    }

    /// Opens a tunnel to `api.github.com:443` like nono does.
    fn client(&self) -> Client {
        Client::open(self.filter.addr(), &self.ca_pem, "api.github.com:443").unwrap()
    }
}

/// A client inside the TLS tunnel.
struct Client {
    /// Buffered TLS stream to the filter.
    io: BufReader<StreamOwned<ClientConnection, TcpStream>>,
}

/// Sends `CONNECT target` and returns the stream and the status of the answer.
fn raw_connect(addr: SocketAddr, target: &str) -> io::Result<(TcpStream, u16)> {
    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(10)))?;
    tcp.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())?;
    let (mut head, mut byte) = (Vec::new(), [0u8; 1]);
    while !head.ends_with(b"\r\n\r\n") {
        if tcp.read(&mut byte)? == 0 {
            break;
        }
        head.extend_from_slice(&byte);
    }
    let status = String::from_utf8_lossy(&head)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((tcp, status))
}

impl Client {
    /// Tunnels to `target`, then does TLS trusting only the Task CA.
    fn open(addr: SocketAddr, ca_pem: &str, target: &str) -> io::Result<Self> {
        let (tcp, status) = raw_connect(addr, target)?;
        if status != 200 {
            return Err(io::Error::other(format!("tunnel refused with {status}")));
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(ca_pem.as_bytes()).unwrap())
            .unwrap();
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
        let name = ServerName::try_from("api.github.com").unwrap();
        let conn = ClientConnection::new(Arc::new(config), name).unwrap();
        Ok(Self {
            io: BufReader::new(StreamOwned::new(conn, tcp)),
        })
    }

    /// Writes bytes into the tunnel.
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let tls = self.io.get_mut();
        tls.write_all(bytes)?;
        tls.flush()
    }

    /// Reads one response, or `None` when the stream ended first.
    fn recv(&mut self) -> Option<Wire> {
        read_message(&mut self.io, true)
    }

    /// Sends a request and reads its response.
    fn call(&mut self, request: &[u8]) -> Wire {
        self.send(request).unwrap();
        self.recv().expect("response")
    }

    /// Whether the peer has closed the stream.
    fn closed(&mut self) -> bool {
        match self.io.fill_buf() {
            Ok(buf) => buf.is_empty(),
            Err(e) => !matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ),
        }
    }
}

/// A GraphQL request body asking for the id of `owner/name`.
fn repo_query(owner: &str, name: &str, operation: &str) -> String {
    let query = format!(
        "query {operation}($owner: String!, $name: String!) {{ \
         repository(owner: $owner, name: $name) {{ id }} }}"
    );
    json!({"query": query, "variables": {"owner": owner, "name": name}}).to_string()
}

/// A mutation that is allowed only if `repository_id` is in the Task repo.
fn create_pr(repository_id: &str) -> String {
    let query = "mutation($input: CreatePullRequestInput!) { \
         createPullRequest(input: $input) { pullRequest { id number url } } }";
    let input = json!({"repositoryId": repository_id, "baseRefName": "main",
        "headRefName": "next", "title": "t", "body": "b"});
    json!({"query": query, "variables": {"input": input}}).to_string()
}

/// A request that carries the credential and a body with a length.
fn request(method: &str, path: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "{method} {path} HTTP/1.1\r\nhost: api.github.com\r\nauthorization: token {TOKEN}\r\n\
         content-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

/// A `POST` with a body of known length.
fn post(path: &str, body: &str) -> Vec<u8> {
    request("POST", path, body.as_bytes())
}

/// A request with a chunked body made of `chunks`.
fn chunked_post(path: &str, chunks: &[&[u8]]) -> Vec<u8> {
    let mut out = format!(
        "POST {path} HTTP/1.1\r\nhost: api.github.com\r\nauthorization: token {TOKEN}\r\n\
         content-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for chunk in chunks {
        out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

/// A response with a JSON body of known length.
fn json_reply(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A chunked response made of `parts`.
fn chunked_reply(parts: &[&str]) -> Vec<u8> {
    let mut out = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
        transfer-encoding: chunked\r\n\r\n"
        .to_vec();
    for part in parts {
        out.extend_from_slice(format!("{:x}\r\n{part}\r\n", part.len()).as_bytes());
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

/// A lookup answer naming `repos` as the repositories of the asked ids.
fn nodes_reply(repos: &[&str]) -> Vec<u8> {
    let nodes: Vec<Value> = repos
        .iter()
        .map(|r| json!({"__typename": "Repository", "nameWithOwner": r}))
        .collect();
    json_reply("200 OK", &json!({"data": {"nodes": nodes}}).to_string())
}

/// Upstream that answers lookups with `lookup` and everything else with `{"data":{"ok":true}}`.
fn standard(lookup: Vec<u8>) -> Responder {
    Arc::new(move |req| {
        if req.is_lookup() {
            lookup.clone()
        } else {
            json_reply("200 OK", OK_BODY)
        }
    })
}

/// Upstream that answers every request with `reply`.
fn fixed(reply: Vec<u8>) -> Responder {
    Arc::new(move |_| reply.clone())
}

/// Asserts a refusal response: status, `moor: ` message, and no leak of the credential.
fn assert_refused(resp: &Wire, status: u16) {
    assert_eq!(resp.status(), status, "{}", resp.head);
    let message = resp.message().expect("json message");
    assert!(message.starts_with("moor: "), "{message}");
    assert!(!String::from_utf8_lossy(&resp.raw).contains(TOKEN));
}
