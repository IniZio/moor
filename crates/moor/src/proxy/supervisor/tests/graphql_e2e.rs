//! `listen` end to end on loopback: a client reaches a fake `api.github.com` through nono and the GraphQL filter.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxId, BoxName, Secret};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::PrivateKeyDer;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use ureq::Agent;

use crate::proxy::nono::tests::upstream;
use crate::proxy::supervisor::listen::{listen, Live};
use crate::proxy::supervisor::{Deps, Sink};
use crate::store::Store;
use crate::testing::bridge::bridge;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::rng::FakeRng;

/// The credential the fake credential source returns.
pub(super) const SECRET: &str = "gh-fake-token-77";
/// What the fake upstream answers to a GraphQL request.
const GRAPHQL_REPLY: &str = r#"{"data":{"repository":{"id":"R_1"}}}"#;
/// What the fake upstream answers to a request without `Authorization`.
const UNAUTHORIZED_REPLY: &str = r#"{"message":"Requires authentication"}"#;
/// What the fake upstream answers to a REST request.
const PULLS_REPLY: &str = r#"[{"number":1}]"#;

/// One request the fake upstream received.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    /// Request line, e.g. `POST /graphql HTTP/1.1`.
    pub(super) line: String,
    /// The request head as received.
    pub(super) head: String,
    /// `Authorization` header value, if any.
    pub(super) authorization: Option<String>,
    /// Request body as text.
    pub(super) body: String,
}

/// Fake `api.github.com`: a TLS server on loopback that records requests.
pub(super) struct Github {
    /// Where it listens.
    pub(super) addr: SocketAddr,
    /// File holding the PEM of the CA that signed its leaf.
    pub(super) ca_path: PathBuf,
    /// Requests received, in order.
    pub(super) seen: Arc<Mutex<Vec<Seen>>>,
    /// Tells the accept loop to end.
    stop: Arc<AtomicBool>,
}

impl Drop for Github {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Server TLS config with an `api.github.com` leaf, and the PEM of its CA.
fn tls_config() -> (Arc<ServerConfig>, String) {
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

/// What the fake upstream answers.
fn reply_for(authorized: bool, request_line: &str) -> &'static str {
    if !authorized {
        UNAUTHORIZED_REPLY
    } else if request_line.starts_with("POST ") {
        GRAPHQL_REPLY
    } else {
        PULLS_REPLY
    }
}

/// Reads one request, records it and answers; the connection then closes.
fn serve_one(tcp: TcpStream, config: Arc<ServerConfig>, seen: &Mutex<Vec<Seen>>) {
    tcp.set_nonblocking(false).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        tcp,
    ));
    let mut head = String::new();
    let mut line = String::new();
    while r.read_line(&mut line).is_ok_and(|n| n > 0) {
        let blank = line == "\r\n";
        head.push_str(&line);
        line.clear();
        if blank {
            break;
        }
    }
    let header = |name: &str| {
        head.split("\r\n")
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
    };
    let len: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let request_line = head.lines().next().unwrap_or_default().to_owned();
    let authorized = header("authorization").as_deref() == Some(&format!("token {SECRET}"));
    let reply = reply_for(authorized, &request_line);
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Seen {
            line: request_line,
            head: head.clone(),
            authorization: header("authorization"),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    let tls = r.get_mut();
    let wire = format!(
        "HTTP/1.1 {} \r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        header("x-reply-status").unwrap_or_else(|| (if authorized { "200" } else { "401" }).to_owned()),
        reply.len()
    );
    if tls
        .write_all(wire.as_bytes())
        .and_then(|()| tls.flush())
        .is_ok()
    {
        tls.conn.send_close_notify();
        let _ = tls.flush();
    }
}

/// Starts the fake upstream; its CA PEM is written under `dir`.
fn start_github(dir: &std::path::Path) -> Github {
    let (config, ca_pem) = tls_config();
    let ca_path = dir.join("github-ca.pem");
    std::fs::write(&ca_path, ca_pem).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (seen2, stop2) = (Arc::clone(&seen), Arc::clone(&stop));
    std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            if let Ok((tcp, _)) = listener.accept() {
                let (config, seen) = (Arc::clone(&config), Arc::clone(&seen2));
                std::thread::spawn(move || serve_one(tcp, config, &seen));
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    Github {
        addr,
        ca_path,
        seen,
        stop,
    }
}

/// The rule that injects the credential into `api.github.com` requests.
pub(super) fn github_rule() -> ResolvedRule {
    let allow = |method: &str, path: &str| EndpointRule {
        method: method.to_owned(),
        path: path.to_owned(),
    };
    ResolvedRule {
        name: "github-api".to_owned(),
        hosts: vec!["api.github.com".to_owned()],
        credential: "GH_TOKEN".to_owned(),
        inject: ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "token {secret}".to_owned(),
        },
        endpoint_rules: vec![allow("POST", "/graphql"), allow("GET", "/repos/o/r/pulls")],
        box_env: BTreeMap::new(),
    }
}

/// A running supervisor listener with its fake upstream and store.
pub(super) struct Rig {
    /// Keeps the state directory alive.
    pub(super) dir: tempfile::TempDir,
    /// Box and event store.
    pub(super) store: Store,
    /// Clock shared with the store.
    clock: Arc<FakeClock>,
    /// Driver the deps need; never used by `listen`.
    driver: FakeDriver,
    /// Randomness the deps need; never used by `listen`.
    rng: FakeRng,
    /// The fake `api.github.com`.
    pub(super) up: Github,
    /// The Box.
    pub(super) box_id: BoxId,
    /// The started nono proxy and filter.
    pub(super) live: Option<Live>,
    /// Port of the GraphQL filter.
    filter_port: u16,
    /// The port the Box uses: a bridge to the front socket, as the reverse tunnel gives it.
    pub(super) box_port: u16,
    /// The private socket directory the front listens in.
    sockets: PathBuf,
    /// PEM of the Box CA the client trusts.
    pub(super) ca_pem: String,
    /// How many times the credential source ran.
    pub(super) fetches: Arc<AtomicUsize>,
}

impl Rig {
    /// The deps `listen` and `drain` take.
    pub(super) fn deps(&self) -> Deps<'_> {
        let err: Sink = Arc::new(Mutex::new(Vec::new()));
        Deps {
            store: &self.store,
            driver: &self.driver,
            forge: Arc::new(FakeForge::new()),
            clock: self.clock.clone(),
            rng: &self.rng,
            creds: {
                let fetches = Arc::clone(&self.fetches);
                Arc::new(move |_: &str| {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(Secret::new(SECRET.as_bytes().to_vec()))
                })
            },
            upstream_ca: Some(self.up.ca_path.clone()),
            upstream_dial: Some(self.up.addr),
            guard_exempt: Vec::new(),
            state_dir: self.dir.path(),
            sockets: &self.sockets,
            owner: rustix::process::getuid().as_raw(),
            boot_id: None,
            stop: Arc::new(AtomicBool::new(false)),
            stderr: err,
        }
    }

    /// Port of the nono proxy.
    pub(super) fn nono_port(&self) -> u16 {
        self.live.as_ref().unwrap().nono_port()
    }

    /// A client that goes through nono as the sandbox does.
    pub(super) fn client(&self) -> Agent {
        upstream::box_client(self.box_port, &self.ca_pem)
    }

    /// Requests the fake upstream received so far.
    pub(super) fn seen(&self) -> Vec<Seen> {
        self.up
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Starts `listen` for a Box on repo `o/r` with the GitHub rule.
fn rig() -> Rig {
    rig_with(Vec::new())
}

/// Like `rig`, with `extra` rules after the GitHub one.
pub(super) fn rig_with(extra: Vec<ResolvedRule>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let up = start_github(dir.path());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: "r".into(),
    };
    let box_id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let spec = ProxySpec {
        box_id,
        box_handle: None,
        host_cwd: "/wt".into(),
        box_repo: Some(ForgeRepo {
            owner: "o".to_owned(),
            name: "r".to_owned(),
        }),
        rules: std::iter::once(github_rule()).chain(extra).collect(),
        user: moor_core::user::UserId::new(1),
        vault_key: "/config/vault.key".into(),
    };
    let sockets = dir.path().join("s");
    let mut rig = Rig {
        dir,
        store,
        clock,
        driver: FakeDriver::new(),
        rng: FakeRng::default(),
        up,
        box_id,
        live: None,
        filter_port: 0,
        box_port: 0,
        sockets,
        ca_pem: String::new(),
        fetches: Arc::new(AtomicUsize::new(0)),
    };
    let live = listen(&rig.deps(), box_id, &spec).unwrap();
    rig.filter_port = live.graphql_addr().unwrap().port();
    rig.box_port = bridge(live.socket());
    rig.ca_pem = crate::proxy::nono::ca::load_or_create(rig.dir.path(), box_id)
        .unwrap()
        .cert_pem()
        .to_owned();
    rig.live = Some(live);
    rig
}

/// POSTs a GraphQL query asking for the id of `owner/name`; status and body.
fn query(client: &Agent, owner: &str, name: &str) -> (u16, String) {
    query_answered(client, owner, name, None)
}

/// Like `query`, with the fake upstream told to answer with `status`.
fn query_answered(client: &Agent, owner: &str, name: &str, status: Option<u16>) -> (u16, String) {
    let q = format!(r#"query{{repository(owner:"{owner}",name:"{name}"){{id}}}}"#);
    let body = serde_json::json!({ "query": q }).to_string();
    let mut request = client
        .post("https://api.github.com/graphql")
        .header("Authorization", "token placeholder-not-real")
        .header("Content-Type", "application/json");
    if let Some(status) = status {
        request = request.header("x-reply-status", status.to_string());
    }
    let mut resp = request.send(body).unwrap();
    let status = resp.status().as_u16();
    (status, resp.body_mut().read_to_string().unwrap())
}

#[test]
fn box_repo_query_reaches_github_with_the_credential() {
    let rig = rig();
    let (status, body) = query(&rig.client(), "o", "r");
    assert_eq!((status, body.as_str()), (200, GRAPHQL_REPLY));
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "POST /graphql HTTP/1.1");
    assert_eq!(
        only.authorization.as_deref(),
        Some("token gh-fake-token-77")
    );
    assert!(only.body.contains(r#"repository(owner:\"o\",name:\"r\")"#));
    rig.live.unwrap().stop();
}

#[test]
fn other_repo_query_reaches_github_without_any_credential() {
    let rig = rig();
    let (status, _) = query(&rig.client(), "victim", "secrets");
    assert_eq!(status, 401, "the fake upstream refuses anonymous requests");
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "POST /graphql HTTP/1.1");
    assert_eq!(only.authorization, None);
    assert!(!only.head.contains(SECRET), "{}", only.head);
    assert!(!only.head.contains("placeholder"), "{}", only.head);
    rig.live.as_ref().unwrap().drain(&rig.deps(), rig.box_id);
    let refused = rig
        .store
        .events(rig.box_id, 100)
        .unwrap()
        .into_iter()
        .any(|e| e.kind == "egress_refused");
    assert!(!refused, "out-of-scope forwarding is not a refusal");
    rig.live.unwrap().stop();
}

#[test]
fn anonymous_401_does_not_evict_the_cached_credential() {
    let rig = rig();
    let (status, _) = query(&rig.client(), "victim", "secrets");
    assert_eq!(status, 401);
    let (status, _) = query(&rig.client(), "victim", "other");
    assert_eq!(status, 401);
    rig.live.as_ref().unwrap().drain(&rig.deps(), rig.box_id);
    let withheld: Vec<String> = rig
        .store
        .events(rig.box_id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "credential_withheld")
        .map(|e| e.payload.to_string())
        .collect();
    let [one] = withheld.as_slice() else {
        unreachable!("one deduplicated event expected: {withheld:?}")
    };
    for want in ["api.github.com", "github-api", "POST", "/graphql"] {
        assert!(one.contains(want), "{want} missing from {one}");
    }
    for leak in ["victim", "secrets", "other", "repository("] {
        assert!(!one.contains(leak), "{leak} leaked into {one}");
    }
    let (status, body) = query(&rig.client(), "o", "r");
    assert_eq!((status, body.as_str()), (200, GRAPHQL_REPLY));
    assert_eq!(
        rig.fetches.load(Ordering::SeqCst),
        1,
        "credential refetched"
    );
    rig.live.unwrap().stop();
}

/// Sends `request` through the front and nono as the Box would, bytes
/// untouched, and returns the response head.
fn raw_call(rig: &Rig, request: &str) -> String {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore};

    let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, rig.box_port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    tcp.write_all(b"CONNECT api.github.com:443 HTTP/1.1\r\nHost: api.github.com:443\r\n\r\n")
        .unwrap();
    let read_head = |r: &mut dyn Read| {
        let (mut head, mut byte) = (Vec::new(), [0u8; 1]);
        while !head.ends_with(b"\r\n\r\n") && r.read(&mut byte).is_ok_and(|n| n == 1) {
            head.push(byte[0]);
        }
        String::from_utf8_lossy(&head).into_owned()
    };
    assert!(read_head(&mut tcp).starts_with("HTTP/1.1 200"));
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(rig.ca_pem.as_bytes()).unwrap())
        .unwrap();
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let name = ServerName::try_from("api.github.com").unwrap();
    let conn = ClientConnection::new(Arc::new(config), name).unwrap();
    let mut tls = StreamOwned::new(conn, tcp);
    tls.write_all(request.as_bytes()).unwrap();
    read_head(&mut tls)
}

#[test]
fn forged_authorization_line_does_not_make_the_credential_refetch() {
    let rig = rig();
    let (status, _) = query(&rig.client(), "o", "r");
    assert_eq!(status, 200);
    let forged = "GET /repos/cli/cli HTTP/1.1\r\nHost: api.github.com\r\n\
                  Authorization : x\r\nConnection: close\r\n\r\n";
    let head = raw_call(&rig, forged);
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    let (status, _) = query(&rig.client(), "o", "r");
    assert_eq!(status, 200);
    assert_eq!(
        rig.fetches.load(Ordering::SeqCst),
        1,
        "credential refetched"
    );
    rig.live.unwrap().stop();
}

#[test]
fn rest_request_passes_through_unchanged() {
    let rig = rig();
    let mut resp = rig
        .client()
        .get("https://api.github.com/repos/o/r/pulls")
        .header("Authorization", "token placeholder-not-real")
        .call()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.body_mut().read_to_string().unwrap(), PULLS_REPLY);
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "GET /repos/o/r/pulls HTTP/1.1");
    assert_eq!(
        only.authorization.as_deref(),
        Some("token gh-fake-token-77")
    );
    assert!(only.body.is_empty());
    rig.live.unwrap().stop();
}

#[test]
fn unmatched_rest_request_is_forwarded_not_refused() {
    let rig = rig();
    let resp = rig
        .client()
        .get("https://api.github.com/repos/cli/cli")
        .header("Authorization", "token placeholder-not-real")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "forwarded without a credential"
    );
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.authorization, None);
    rig.live.unwrap().stop();
}

#[test]
fn stop_closes_every_listener() {
    let mut rig = rig();
    let socket = rig.live.as_ref().unwrap().socket().to_path_buf();
    let ports = [rig.nono_port(), rig.filter_port];
    rig.live.take().unwrap().stop();
    assert!(UnixStream::connect(&socket).is_err(), "front still accepts");
    for port in ports {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        assert!(
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err(),
            "port {port} still accepts connections"
        );
    }
}

mod rejected;
