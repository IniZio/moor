//! The gate over TLS with the front hello, against a fake controller that goes away and comes
//! back on the same port: requests that provably never ran are held and sent once, the rest
//! get 503, and a request cut after it was sent is never sent again.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moor_client::tls::{bridge, TlsPki};
use moor_client::Redial;
use moor_core::controller::{FrameErrorKind, ServerFrame};
use moor_core::response::Response;
use moor_gate::{over_tls, Gate, Mcp, Verifier, HOLD};
use moor_testkit::oidc::{now, Issuer};
use moor_testkit::LockedPort;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer as CertIssuer, KeyPair, SanType};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};
use serde_json::{json, Value};
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

use super::{sign, CLIENT_ID};

mod cap;
mod survives;

const FRONT_KEY: &str = "gate-front-key";
const CUT: &str = "repo/cut";

/// A hold no test here runs out unless it means to.
const LONG: Duration = Duration::from_secs(10);

/// [`HOLD`] with `budget` in place of its 30 s, so a test need not wait them out.
const fn holding(budget: Duration) -> Redial {
    Redial {
        budget,
        refused: budget,
        ..HOLD
    }
}

/// Writes a CA and the certificates both ends present; gives the gate's files and the TLS config
/// of a listener that asks for a client certificate from that CA.
fn pki(dir: &Path) -> (TlsPki, Arc<ServerConfig>) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = CertIssuer::new(ca, ca_key);
    let server_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![SanType::IpAddress([127, 0, 0, 1].into())];
    let server = params.signed_by(&server_key, &issuer).unwrap();
    let client_key = KeyPair::generate().unwrap();
    let client = CertificateParams::default()
        .signed_by(&client_key, &issuer)
        .unwrap();
    let files = TlsPki {
        ca: dir.join("ca.pem"),
        cert: dir.join("client.pem"),
        key: dir.join("client.key"),
    };
    std::fs::write(&files.ca, ca_cert.pem()).unwrap();
    std::fs::write(&files.cert, client.pem()).unwrap();
    std::fs::write(&files.key, client_key.serialize_pem()).unwrap();
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca_cert.pem().as_bytes()).unwrap())
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(roots.into(), provider.clone())
        .build()
        .unwrap();
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(server.pem().as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(server_key.serialize_pem().as_bytes()).unwrap(),
        )
        .unwrap();
    (files, Arc::new(config))
}

/// What the fake controller answers a hello with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Hello,
    Restarting,
    /// Takes the connection and says nothing, as a controller coming up may.
    Silent,
}

/// A fake controller's state, which outlives each time it is up.
struct Fake {
    answer: Mutex<Answer>,
    /// How many times each Box was shown.
    shown: Mutex<HashMap<String, usize>>,
    config: Arc<ServerConfig>,
}

impl Fake {
    fn shown(&self, name: &str) -> usize {
        self.shown
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or_default()
    }
}

/// One line read a byte at a time, leaving the rest of the stream unread.
fn line(tls: &mut impl Read) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0; 1];
    loop {
        if tls.read(&mut byte).ok()? == 0 {
            return None;
        }
        if byte == *b"\n" {
            return String::from_utf8(line).ok();
        }
        line.push(byte[0]);
    }
}

/// Serves one connection: the TLS hello, then the controller socket's protocol.
fn serve_one(mut tcp: TcpStream, fake: &Fake) {
    if *fake.answer.lock().unwrap() == Answer::Silent {
        let _ = std::io::copy(&mut tcp, &mut std::io::sink());
        return;
    }
    let conn = ServerConnection::new(Arc::clone(&fake.config)).unwrap();
    let mut tls = StreamOwned::new(conn, tcp);
    let Some(hello) = line(&mut tls) else {
        return;
    };
    let hello: Value = serde_json::from_str(&hello).unwrap();
    if hello["front"]["ns"] != "authgear" || hello["front"]["key"] != FRONT_KEY {
        let _ = writeln!(tls, r#"{{"Err":{{"refused":"not the key of front"}}}}"#);
        let _ = tls.flush();
        return;
    }
    writeln!(tls, r#"{{"Ok":null}}"#).unwrap();
    tls.flush().unwrap();
    let near = bridge(tls).unwrap();
    let mut reply = near.try_clone().unwrap();
    for frame in BufReader::new(near).lines().map_while(Result::ok) {
        let frame: Value = serde_json::from_str(&frame).unwrap();
        if frame["frame"] == "hello" {
            let answer = *fake.answer.lock().unwrap();
            let out = match answer {
                Answer::Hello | Answer::Silent => ServerFrame::Hello {
                    protocol: frame["protocol"].as_u64().unwrap().try_into().unwrap(),
                },
                Answer::Restarting => ServerFrame::Error {
                    id: None,
                    kind: FrameErrorKind::Restarting,
                    message: "restarting".to_owned(),
                },
            };
            let _ = reply.write_all(out.to_line().as_bytes());
            continue;
        }
        let inner = &frame["request"]["request"];
        let name = inner["box"].as_str();
        if let (Some("show"), Some(name)) = (inner["cmd"].as_str(), name) {
            *fake
                .shown
                .lock()
                .unwrap()
                .entry(name.to_owned())
                .or_default() += 1;
            if name == CUT {
                return;
            }
        }
        let value = name.map_or(
            json!({}),
            |name| json!({"box": {"name": name, "lifecycle": "ready"}, "events": []}),
        );
        let envelope = Response::ok("x", Some(value));
        let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
        let _ = writeln!(reply, "{out}");
    }
}

/// A fake controller's listener, which refuses connects once it is stopped.
struct Up {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// Starts accepting on `listener`.
fn start(listener: TcpListener, fake: &Arc<Fake>) -> Up {
    let stop = Arc::new(AtomicBool::new(false));
    let (stopped, fake) = (Arc::clone(&stop), Arc::clone(fake));
    listener.set_nonblocking(true).unwrap();
    let thread = std::thread::spawn(move || {
        while !stopped.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((tcp, _)) => {
                    tcp.set_nonblocking(false).unwrap();
                    let fake = Arc::clone(&fake);
                    std::thread::spawn(move || serve_one(tcp, &fake));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Up { stop, thread }
}

impl Up {
    /// Closes the listener, so the next connect is refused.
    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        self.thread.join().unwrap();
    }
}

/// A gate over TLS in front of a fake controller.
struct Held {
    gate: String,
    fake: Arc<Fake>,
    port: u16,
    up: Option<Up>,
    resource: String,
    issuer: Issuer,
    agent: ureq::Agent,
    _port: LockedPort,
    _dir: tempfile::TempDir,
}

fn held_with(key: &str, hold: Redial) -> Held {
    held_served(key, hold, |listener, gate| {
        std::thread::spawn(move || moor_gate::serve(&listener, &gate));
    })
}

/// A gate over TLS that `serve` starts serving.
fn held_served(key: &str, hold: Redial, serve: impl FnOnce(TcpListener, Arc<Gate>)) -> Held {
    let dir = tempfile::tempdir().unwrap();
    let (files, config) = pki(dir.path());
    let (listener, lock) = LockedPort::listen();
    let port = lock.port();
    let fake = Arc::new(Fake {
        answer: Mutex::new(Answer::Hello),
        shown: Mutex::default(),
        config,
    });
    let up = start(listener, &fake);
    let controller = over_tls(&format!("tls://127.0.0.1:{port}"), &files, key, hold).unwrap();
    let issuer = Issuer::start(CLIENT_ID);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = listener.local_addr().unwrap().to_string();
    let resource = format!("http://{gate}/mcp");
    let config = Arc::new(Gate {
        mcp: Some(Mcp::new(&resource, controller.clone()).unwrap()),
        controller,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    serve(listener, config);
    Held {
        gate,
        fake,
        port,
        up: Some(up),
        resource,
        issuer,
        agent: ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into(),
        _port: lock,
        _dir: dir,
    }
}

/// How a call to the gate ended.
#[derive(Debug)]
struct Answered {
    status: u16,
    retry_after: Option<String>,
    body: Value,
    session: Option<String>,
}

impl Held {
    fn away(&mut self) {
        self.up.take().unwrap().stop();
    }

    fn up_again(&self) -> Up {
        start(
            TcpListener::bind(("127.0.0.1", self.port)).unwrap(),
            &self.fake,
        )
    }

    fn token(&self, aud: &str) -> String {
        let claims = json!({"iss": self.issuer.base, "aud": aud, "sub": "alice-sub",
            "exp": now() + 600, "email": "alice@example.com", "email_verified": true,
            "scope": "moor:read moor:write"});
        sign(&self.issuer.key, &claims)
    }

    fn post(&self, body: &Value, session: Option<&str>) -> Answered {
        let mut req = self
            .agent
            .post(&self.resource)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header(
                "Authorization",
                format!("Bearer {}", self.token(&self.resource)),
            );
        if let Some(session) = session {
            req = req.header("Mcp-Session-Id", session);
        }
        let mut res = req.send(body.to_string()).unwrap();
        let header = |name: &str| {
            res.headers()
                .get(name)
                .map(|v| v.to_str().unwrap().to_owned())
        };
        let (retry_after, session) = (header("retry-after"), header("mcp-session-id"));
        let status = res.status().as_u16();
        let text = res.body_mut().read_to_string().unwrap();
        Answered {
            status,
            retry_after,
            body: serde_json::from_str(&text).unwrap_or(Value::Null),
            session,
        }
    }

    fn initialize(&self) -> Answered {
        self.post(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}}}),
            None,
        )
    }

    /// An open MCP session, when the server keeps one.
    fn session(&self) -> Option<String> {
        let opened = self.initialize();
        assert_eq!(opened.status, 200, "{opened:?}");
        self.post(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            opened.session.as_deref(),
        );
        opened.session
    }

    fn status_of(&self, session: Option<&str>, name: &str) -> Answered {
        self.post(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "box_status", "arguments": {"box": name}}}),
            session,
        )
    }

    /// Asks for `/front`, and gives the socket and the response head.
    fn front(&self) -> (TcpStream, String) {
        let token = self.token(CLIENT_ID);
        let mut stream = TcpStream::connect(&self.gate).unwrap();
        write!(
            stream,
            "GET /front HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .unwrap();
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).unwrap(), 1, "{head:?}");
            head.push(byte[0]);
        }
        (stream, String::from_utf8(head).unwrap())
    }
}

fn answered_ok(call: &Answered) {
    assert_eq!(call.status, 200, "{call:?}");
    assert_ne!(call.body["result"]["isError"], true, "{call:?}");
    assert!(call.body["result"].is_object(), "{call:?}");
}

/// A protocol hello through the relayed `/front` reaches the controller and comes back.
fn relays(stream: TcpStream) {
    let mut ws = WebSocket::from_raw_socket(stream, Role::Client, None);
    let hello = moor_core::controller::ClientFrame::Hello {
        protocol: moor_core::controller::PROTOCOL,
    };
    ws.send(Message::binary(hello.to_line().into_bytes()))
        .unwrap();
    let Message::Binary(reply) = ws.read().unwrap() else {
        panic!("the relay answered something else");
    };
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["frame"], "hello", "{reply}");
}

#[test]
fn the_gate_holds_30_s_on_every_way_a_request_may_not_have_run() {
    assert_eq!(HOLD.budget, Duration::from_secs(30));
    assert_eq!(HOLD.refused, HOLD.budget);
}

#[test]
fn a_controller_away_within_the_hold_leaves_every_call_answered_with_none_run_twice() {
    let gap = Duration::from_secs(2);
    let mut w = held_with(FRONT_KEY, holding(LONG));
    let session = w.session();
    answered_ok(&w.status_of(session.as_deref(), "repo/before"));
    w.away();
    let began = Instant::now();
    std::thread::scope(|scope| {
        let w = &w;
        let session = &session;
        let calls: Vec<_> = (0..10_u32)
            .map(|i| {
                scope.spawn(move || {
                    std::thread::sleep(gap * i / 10);
                    let name = format!("repo/{i}");
                    (
                        w.status_of(session.as_deref(), &name),
                        name,
                        began.elapsed(),
                    )
                })
            })
            .collect();
        let opened = scope.spawn(|| w.initialize());
        let front = scope.spawn(|| w.front());
        std::thread::sleep(gap);
        let up = w.up_again();
        for call in calls {
            let (call, name, at) = call.join().unwrap();
            answered_ok(&call);
            assert!(at >= gap, "{name} answered at {at:?}");
            assert_eq!(w.fake.shown(&name), 1, "{name} ran other than once");
        }
        assert_eq!(opened.join().unwrap().status, 200);
        let (stream, head) = front.join().unwrap();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        relays(stream);
        up.stop();
    });
}

#[test]
fn a_controller_away_past_the_hold_gets_503_with_retry_after_when_it_runs_out() {
    let hold = Duration::from_secs(2);
    let mut w = held_with(FRONT_KEY, holding(hold));
    let session = w.session();
    w.away();
    let began = Instant::now();
    let in_time = |at: Duration| {
        assert!(
            (hold..hold + Duration::from_secs(3)).contains(&at),
            "answered at {at:?}"
        );
    };
    std::thread::scope(|scope| {
        let call = scope.spawn(|| {
            let call = w.status_of(session.as_deref(), "repo/late");
            (call, began.elapsed())
        });
        let front = scope.spawn(|| {
            let got = w.front();
            (got, began.elapsed())
        });
        let (call, at) = call.join().unwrap();
        assert_eq!(call.status, 503, "{call:?}");
        assert_eq!(call.retry_after.as_deref(), Some("5"), "{call:?}");
        in_time(at);
        let ((_, head), at) = front.join().unwrap();
        assert!(head.starts_with("HTTP/1.1 503"), "{head}");
        assert!(head.contains("Retry-After: 5\r\n"), "{head}");
        in_time(at);
    });
    assert_eq!(w.fake.shown("repo/late"), 0);
}

#[test]
fn a_restarting_answer_is_held_like_a_refused_connect() {
    let w = held_with(FRONT_KEY, holding(LONG));
    let session = w.session();
    *w.fake.answer.lock().unwrap() = Answer::Restarting;
    let began = Instant::now();
    std::thread::scope(|scope| {
        let call = scope.spawn(|| w.status_of(session.as_deref(), "repo/draining"));
        std::thread::sleep(Duration::from_secs(1));
        *w.fake.answer.lock().unwrap() = Answer::Hello;
        answered_ok(&call.join().unwrap());
    });
    assert!(began.elapsed() >= Duration::from_secs(1));
    assert_eq!(w.fake.shown("repo/draining"), 1);
}

/// The rule the hold rests on: only a request that provably never ran is sent again. Once it was
/// written and the connection then ended, it may have run, so the client hears of the failure.
#[test]
fn a_request_cut_after_send_is_an_error_and_is_never_replayed() {
    let w = held_with(FRONT_KEY, holding(LONG));
    let session = w.session();
    let began = Instant::now();
    let cut = w.status_of(session.as_deref(), CUT);
    assert_eq!(cut.status, 200, "{cut:?}");
    assert_eq!(cut.body["result"]["isError"], true, "{cut:?}");
    assert!(began.elapsed() < Duration::from_secs(5), "it was held");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        w.fake.shown(CUT),
        1,
        "a request that may have run was sent again"
    );
    answered_ok(&w.status_of(session.as_deref(), "repo/after"));
}

#[test]
fn a_gate_with_a_wrong_front_key_is_refused_at_once() {
    let w = held_with("not-the-front-key", holding(LONG));
    let began = Instant::now();
    let opened = w.initialize();
    assert_eq!(opened.status, 502, "{opened:?}");
    assert_eq!(opened.retry_after, None, "{opened:?}");
    let (_, head) = w.front();
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(began.elapsed() < Duration::from_secs(5), "it was held");
}

#[test]
fn readiness_does_not_wait_for_the_controller() {
    let mut w = held_with(FRONT_KEY, holding(LONG));
    w.away();
    let began = Instant::now();
    let mut res = w
        .agent
        .get(format!("http://{}/.well-known/moor", w.gate))
        .call()
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert!(began.elapsed() < Duration::from_secs(5), "it was held");
    let doc: Value = serde_json::from_str(&res.body_mut().read_to_string().unwrap()).unwrap();
    assert_eq!(doc["client_id"], CLIENT_ID);
}
