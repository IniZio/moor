//! `listen` end to end on loopback: a client reaches a fake `api.github.com` through nono and the GraphQL filter.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{Secret, TaskId, TaskName};
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
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;
use crate::testing::rng::FakeRng;

/// The Task Nonce the client uses as proxy password.
pub(super) const NONCE: &str = "nonce_gql-0123456789";
/// The credential the fake credential source returns.
pub(super) const SECRET: &str = "gh-fake-token-77";
/// Longest the test waits for the drain.
const DEADLINE: Duration = Duration::from_secs(20);
/// What the fake upstream answers to a GraphQL request.
const GRAPHQL_REPLY: &str = r#"{"data":{"repository":{"id":"R_1"}}}"#;
/// What the fake upstream answers to a REST request.
const PULLS_REPLY: &str = r#"[{"number":1}]"#;

/// One request the fake upstream received.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    /// Request line, e.g. `POST /graphql HTTP/1.1`.
    pub(super) line: String,
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
    let reply = if request_line.starts_with("POST ") {
        GRAPHQL_REPLY
    } else {
        PULLS_REPLY
    };
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Seen {
            line: request_line,
            authorization: header("authorization"),
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    let tls = r.get_mut();
    let wire = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
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
            header: "authorization".to_owned(),
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
    /// Task and event store.
    pub(super) store: Store,
    /// Clock shared with the store.
    clock: Arc<FakeClock>,
    /// Driver the deps need; never used by `listen`.
    driver: FakeDriver,
    /// Randomness the deps need; never used by `listen`.
    rng: FakeRng,
    /// The fake `api.github.com`.
    pub(super) up: Github,
    /// The Task.
    pub(super) task: TaskId,
    /// The started nono proxy and filter.
    pub(super) live: Option<Live>,
    /// Port of the GraphQL filter.
    filter_port: u16,
    /// PEM of the Task CA the client trusts.
    pub(super) ca_pem: String,
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
            creds: Arc::new(|_: &str| Ok(Secret::new(SECRET.as_bytes().to_vec()))),
            upstream_ca: Some(self.up.ca_path.clone()),
            upstream_dial: Some(self.up.addr),
            guard_exempt: Vec::new(),
            state_dir: self.dir.path(),
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
        upstream::client(self.live.as_ref().unwrap().port(), NONCE, &self.ca_pem)
    }

    /// Requests the fake upstream received so far.
    pub(super) fn seen(&self) -> Vec<Seen> {
        self.up
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Drains until an `egress_refused` payload shows up; its JSON text.
    fn refusal(&self) -> String {
        let step = Duration::from_millis(50);
        for _ in 0..DEADLINE.as_millis() / step.as_millis() {
            self.live.as_ref().unwrap().drain(&self.deps(), self.task);
            let found = self
                .store
                .events(self.task, 100)
                .unwrap()
                .into_iter()
                .find(|e| e.kind == "egress_refused");
            if let Some(e) = found {
                return e.payload.to_string();
            }
            std::thread::sleep(step);
        }
        unreachable!("timed out waiting for an egress_refused event")
    }
}

/// Starts `listen` for a Task on repo `o/r` with the GitHub rule.
fn rig() -> Rig {
    rig_with(Vec::new())
}

/// Like `rig`, with `extra` rules after the GitHub one.
pub(super) fn rig_with(extra: Vec<ResolvedRule>) -> Rig {
    rig_allowing(extra, Vec::new())
}

/// Like `rig_with`, with `allow` as the Box egress allow-list.
pub(super) fn rig_allowing(extra: Vec<ResolvedRule>, allow: Vec<String>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let up = start_github(dir.path());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let nt = NewTask {
        name: TaskName::parse("demo").unwrap(),
        repo: "r".into(),
    };
    let task = store.apply(Decision::insert(nt)).unwrap().task;
    let spec = ProxySpec {
        task,
        box_handle: None,
        host_cwd: "/wt".into(),
        task_repo: Some(ForgeRepo {
            owner: "o".to_owned(),
            name: "r".to_owned(),
        }),
        rules: std::iter::once(github_rule()).chain(extra).collect(),
        allow,
    };
    let mut rig = Rig {
        dir,
        store,
        clock,
        driver: FakeDriver::new(),
        rng: FakeRng::default(),
        up,
        task,
        live: None,
        filter_port: 0,
        ca_pem: String::new(),
    };
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    let live = listen(&rig.deps(), task, &nonce, &spec).unwrap();
    rig.filter_port = live.graphql_addr().unwrap().port();
    rig.ca_pem = crate::proxy::nono::ca::load_or_create(rig.dir.path(), task)
        .unwrap()
        .cert_pem()
        .to_owned();
    rig.live = Some(live);
    rig
}

/// POSTs a GraphQL query asking for the id of `owner/name`; status and body.
fn query(client: &Agent, owner: &str, name: &str) -> (u16, String) {
    let q = format!(r#"query{{repository(owner:"{owner}",name:"{name}"){{id}}}}"#);
    let body = serde_json::json!({ "query": q }).to_string();
    let mut resp = client
        .post("https://api.github.com/graphql")
        .header("Authorization", "token placeholder-not-real")
        .header("Content-Type", "application/json")
        .send(body)
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.body_mut().read_to_string().unwrap())
}

#[test]
fn task_repo_query_reaches_github_with_the_credential() {
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
fn other_repo_query_is_refused_audited_and_never_forwarded() {
    let rig = rig();
    let (status, body) = query(&rig.client(), "victim", "secrets");
    assert_eq!(status, 403);
    let message: serde_json::Value = serde_json::from_str(&body).unwrap();
    let text = message
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    assert!(text.starts_with("moor: "), "{text}");
    assert!(!body.contains(SECRET));
    assert!(rig.seen().is_empty());

    let event = rig.refusal();
    for want in ["api.github.com", "graphql_scope", "POST"] {
        assert!(event.contains(want), "{want} missing from {event}");
    }
    for leak in [SECRET, "victim", "secrets", "repository("] {
        assert!(!event.contains(leak), "{leak} leaked into {event}");
    }
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
fn unmatched_rest_request_passes_through_without_credential() {
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
    let status = resp.status().as_u16();
    let seen = rig.seen();
    assert_eq!(status, 200, "seen={seen:?}");
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "GET /repos/cli/cli HTTP/1.1");
    assert_eq!(only.authorization, None);
    rig.live.unwrap().stop();
}

#[test]
fn stop_closes_both_listeners() {
    let mut rig = rig();
    let ports = [
        rig.live.as_ref().unwrap().port(),
        rig.nono_port(),
        rig.filter_port,
    ];
    rig.live.take().unwrap().stop();
    for port in ports {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        assert!(
            TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err(),
            "port {port} still accepts connections"
        );
    }
}
