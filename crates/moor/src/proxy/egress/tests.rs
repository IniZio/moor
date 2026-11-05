//! The shared egress Proxy end to end on loopback: a WebSocket per CONNECT, keyed by the Box
//! token, through the Box's proxy to a fake HTTPS upstream, with the ledger served as the
//! controller serves it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::Identity;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{self, Message, WebSocket};
use zeroize::Zeroizing;

use super::boxes::closed;
use super::wire::authority_ok;
use super::{Config, Rate, Server, PATH, TARGET};
use crate::proxy::ledger::tests::{serve_on, Served, EGRESS_KEY};
use crate::proxy::ledger::{BoxView, Endpoint, LedgerError};
use crate::proxy::nono::ca;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::proxy::token;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{put_secret, test_vault_key};
use crate::testing::rng::FakeRng;
use crate::vault::Vault;

/// The credential the Box user holds, which only the upstream may see.
const SECRET: &str = "test-token-1";

/// A controller's store and ledger, an upstream, a Box with a token, and the shared Proxy.
struct World {
    dir: tempfile::TempDir,
    store: Store,
    up: Upstream,
    id: BoxId,
    token: String,
    log: Arc<Mutex<Vec<u8>>>,
    ledger: Option<Served>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let up = upstream::start(dir.path());
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let vault = Vault::load(&test_vault_key(dir.path())).unwrap();
        let (user, _) = store.enroll(&Identity::local(1), None).unwrap();
        put_secret(&store, &vault, user, "tool-token", SECRET);
        let id = ready_box(&store, user);
        drop(store.reserve(id, &FakeRng::default(), crate::proxy::detached::LEASE));
        let token = token::mint(&store, EGRESS_KEY.as_bytes(), id)
            .unwrap()
            .unwrap();
        let token = String::from_utf8(token.expose().to_vec()).unwrap();
        let spec = ProxySpec {
            box_id: id,
            box_handle: None,
            box_repo: None,
            other_repos: Vec::new(),
            rules: vec![rule(up.port)],
            user,
        };
        store.set_proxy_spec(id, &spec).unwrap();
        let socket = dir.path().join("proxy.sock");
        let ledger = serve_on(&socket, store.clone(), Arc::new(vault), None);
        Self {
            dir,
            store,
            up,
            id,
            token,
            log: Arc::default(),
            ledger: Some(ledger),
        }
    }

    /// The shared Proxy, letting each token open `burst` tunnels and no more.
    fn server(&self, burst: f64) -> Server {
        self.server_with(burst, 10.0)
    }

    /// [`World::server`], with `lookups` lookups for tokens not yet confirmed.
    fn server_with(&self, burst: f64, lookups: f64) -> Server {
        self.server_full(burst, lookups, 64)
    }

    /// [`World::server_with`], letting each token hold `tunnels` tunnels open.
    fn server_full(&self, burst: f64, lookups: f64, tunnels: usize) -> Server {
        Server::start(Config {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            ledger: Endpoint::Unix(self.dir.path().join("proxy.sock")),
            key: Zeroizing::new(EGRESS_KEY.to_owned()),
            work_dir: self.dir.path().join("egress"),
            rate: Rate {
                per_sec: 0.0,
                burst,
            },
            lookups: Rate {
                per_sec: 0.0,
                burst: lookups,
            },
            tunnels,
            tend: Duration::from_millis(50),
            log: self.log.clone(),
            upstream_ca: Some(self.up.ca_path.clone()),
            guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, self.up.port))],
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
        })
        .unwrap()
    }

    fn target(&self) -> String {
        format!("localhost:{}", self.up.port)
    }

    fn close_box(&self) {
        let d = Decision::new(self.id)
            .lifecycle(BoxLifecycleInput::Close { reason: None })
            .session(SessionInput::End)
            .box_input(BoxInput::Released);
        self.store.apply(d).unwrap();
    }

    /// A GET of `/ok` over `tunnel`, as the Box sends it; the upstream's status line.
    fn get(&self, tunnel: WebSocket<TcpStream>) -> String {
        let ca = self.store.proxy_ca(self.id).unwrap().unwrap().cert_pem;
        let mut tls = tls_over(tunnel, &ca);
        let req = format!(
            "GET /ok HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer placeholder\r\n\
             Connection: close\r\n\r\n",
            self.target()
        );
        tls.write_all(req.as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n") && tls.read(&mut byte).unwrap() == 1 {
            head.extend_from_slice(&byte);
        }
        String::from_utf8(head).unwrap().trim_end().to_owned()
    }
}

/// Inserts a Box for `user` and takes it to Ready.
fn ready_box(store: &Store, user: moor_core::user::UserId) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
    };
    let id = store
        .apply(Decision::insert(new).requester(user))
        .unwrap()
        .box_id;
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admitted).unwrap();
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("p/demo"),
        });
    store.apply(ready).unwrap();
    id
}

/// `GET /ok` on `localhost:<port>`, with the `tool-token` secret as a bearer token.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "tool".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "tool-token".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// Opens a tunnel to `target` as the forwarder does; the HTTP status of a refusal.
fn open(server: &Server, token: &str, target: &str) -> Result<WebSocket<TcpStream>, u16> {
    let url = format!("ws://{}{PATH}", server.addr());
    let mut req = url.into_client_request().unwrap();
    let headers = req.headers_mut();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    headers.insert(TARGET, target.parse().unwrap());
    let stream = TcpStream::connect(server.addr()).map_err(|_| 0_u16)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    match tungstenite::client(req, stream) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))) => {
            Err(resp.status().as_u16())
        }
        Err(e) => unreachable!("handshake failed: {e}"),
    }
}

/// The tunnel's bytes as a stream, for TLS to run over.
struct WsIo {
    ws: WebSocket<TcpStream>,
    buf: Vec<u8>,
}

impl Read for WsIo {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.buf.is_empty() {
            match self.ws.read() {
                Ok(Message::Binary(b)) => self.buf = b.to_vec(),
                Ok(Message::Close(_)) | Err(_) => return Ok(0),
                Ok(_) => {}
            }
        }
        let n = out.len().min(self.buf.len());
        for (to, from) in out.iter_mut().zip(self.buf.drain(..n)) {
            *to = from;
        }
        Ok(n)
    }
}

impl Write for WsIo {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.ws
            .send(Message::binary(data.to_vec()))
            .map_err(std::io::Error::other)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.ws.flush().map_err(std::io::Error::other)
    }
}

/// A TLS client over `ws` that trusts only the Box CA `ca_pem`, for `localhost`.
fn tls_over(ws: WebSocket<TcpStream>, ca_pem: &str) -> StreamOwned<ClientConnection, WsIo> {
    let mut roots = RootCertStore::empty();
    let der = rustls_pemfile_der(ca_pem);
    roots.add(der).unwrap();
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let name = ServerName::try_from("localhost").unwrap();
    let conn = ClientConnection::new(Arc::new(config), name).unwrap();
    StreamOwned::new(
        conn,
        WsIo {
            ws,
            buf: Vec::new(),
        },
    )
}

/// The one certificate in `pem`.
fn rustls_pemfile_der(pem: &str) -> CertificateDer<'static> {
    use rustls::pki_types::pem::PemObject as _;
    CertificateDer::from_pem_slice(pem.as_bytes()).unwrap()
}

#[test]
fn a_valid_token_reaches_the_upstream_with_the_credential_injected() {
    let w = World::new();
    let server = w.server(10.0);
    let tunnel = open(&server, &w.token, &w.target()).unwrap();
    assert_eq!(w.get(tunnel), "HTTP/1.1 200 OK");
    let seen = w.up.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.authorization.as_deref(), Some("Bearer test-token-1"));
    assert_eq!(server.drain(Duration::from_secs(5)), 0);
}

#[test]
fn a_wrong_or_missing_token_is_refused() {
    let w = World::new();
    let server = w.server(10.0);
    assert_eq!(open(&server, "not-the-token", &w.target()).err(), Some(401));
    assert_eq!(open(&server, "", &w.target()).err(), Some(401));
    assert_eq!(open(&server, &w.token, "no-port").err(), Some(400));
    assert!(w.up.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn the_token_of_a_closed_box_is_refused() {
    let w = World::new();
    let server = w.server(10.0);
    assert!(open(&server, &w.token, &w.target()).is_ok());
    w.close_box();
    assert_eq!(open(&server, &w.token, &w.target()).err(), Some(401));
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_token_past_its_rate_is_refused() {
    let w = World::new();
    let server = w.server(2.0);
    assert!(open(&server, &w.token, &w.target()).is_ok());
    assert!(open(&server, &w.token, &w.target()).is_ok());
    assert_eq!(open(&server, &w.token, &w.target()).err(), Some(429));
    assert_eq!(
        open(&server, "another-token", &w.target()).err(),
        Some(401),
        "each token has its own rate"
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_drain_lets_an_open_tunnel_finish_and_refuses_a_new_one() {
    let w = World::new();
    let server = w.server(10.0);
    let addr = server.addr();
    let tunnel = open(&server, &w.token, &w.target()).unwrap();
    std::thread::scope(|s| {
        let drained = s.spawn(|| server.drain(Duration::from_secs(20)));
        let mut tries = 0;
        while TcpStream::connect(addr).is_ok() {
            tries += 1;
            assert!(tries < 200, "still listening while draining");
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            w.get(tunnel),
            "HTTP/1.1 200 OK",
            "the open tunnel still works"
        );
        assert_eq!(drained.join().unwrap(), 0, "it finished within the grace");
    });
    assert_eq!(w.up.seen().len(), 1);
}

/// A token with a valid MAC for an `id` no Box holds.
fn signed(id: &str) -> String {
    use hmac::{Hmac, Mac as _};
    let mut m = Hmac::<sha2::Sha256>::new_from_slice(EGRESS_KEY.as_bytes()).unwrap();
    m.update(id.as_bytes());
    let mac: String = m
        .finalize()
        .into_bytes()
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
            out
        });
    format!("{id}.{mac}")
}

/// Polls `done` for up to ten seconds.
fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    unreachable!("timed out waiting for {what}");
}

#[test]
fn unsigned_tokens_are_refused_before_they_spend_any_budget() {
    let w = World::new();
    let server = w.server_with(10.0, 1.0);
    for n in 0..20 {
        let forged = format!("random-{n}.{}", "0".repeat(64));
        assert_eq!(open(&server, &forged, &w.target()).err(), Some(401));
    }
    assert!(
        open(&server, &w.token, &w.target()).is_ok(),
        "the one lookup is still there for a signed token"
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn signed_unknown_tokens_are_never_kept_and_spend_only_the_shared_lookup_budget() {
    let w = World::new();
    let server = w.server_with(10.0, 4.0);
    assert!(
        open(&server, &w.token, &w.target()).is_ok(),
        "the first lookup"
    );
    let refused: Vec<Option<u16>> = (0..20)
        .map(|n| open(&server, &signed(&format!("id-{n}")), &w.target()).err())
        .collect();
    let mut want = vec![Some(401); 3];
    want.resize(20, Some(429));
    assert_eq!(refused, want, "the rest of the lookup budget, then 429");
    assert_eq!(server.tracked(), 1, "only the confirmed token is kept");
    assert!(
        open(&server, &w.token, &w.target()).is_ok(),
        "a confirmed token draws on its own bucket"
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_token_holds_at_most_its_cap_of_open_tunnels() {
    let w = World::new();
    let server = w.server_full(10.0, 10.0, 2);
    let first = open(&server, &w.token, &w.target()).unwrap();
    let second = open(&server, &w.token, &w.target()).unwrap();
    assert_eq!(open(&server, &w.token, &w.target()).err(), Some(429));
    drop((first, second));
    eventually("a freed tunnel", || {
        open(&server, &w.token, &w.target()).is_ok()
    });
    server.drain(Duration::from_secs(5));
}

#[test]
fn tending_stops_the_proxy_of_a_closed_box_and_removes_its_ca() {
    let w = World::new();
    let server = w.server(10.0);
    drop(open(&server, &w.token, &w.target()).unwrap());
    assert_eq!(server.serving(), 1);
    let ca_dir = ca::ca_dir(&w.dir.path().join("egress"), w.id);
    assert!(ca_dir.exists());
    w.close_box();
    eventually("the proxy to stop and its CA key to go", || {
        server.serving() == 0 && !ca_dir.exists()
    });
    server.drain(Duration::from_secs(5));
}

#[test]
fn only_a_closed_or_missing_box_counts_as_closed() {
    let view = |open| {
        Ok(Some(BoxView {
            open,
            handle: None,
            asleep: false,
        }))
    };
    assert!(closed(&Ok(None)));
    assert!(closed(&view(false)));
    assert!(!closed(&view(true)));
    for e in [
        LedgerError::Store("disk".to_owned()),
        LedgerError::Refused("no".to_owned()),
        LedgerError::Unavailable("gone".to_owned()),
    ] {
        assert!(!closed(&Err(e)), "a failed read keeps the Box served");
    }
}

#[test]
fn a_changed_spec_rebuilds_the_proxy() {
    let w = World::new();
    let server = w.server(10.0);
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    let mut spec = w.store.proxy_spec(w.id).unwrap().unwrap();
    for r in &mut spec.rules {
        r.inject = ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Token {secret}".to_owned(),
        };
    }
    w.store.set_proxy_spec(w.id, &spec).unwrap();
    eventually("the new spec's credential form", || {
        let tunnel = open(&server, &w.token, &w.target()).unwrap();
        drop(w.get(tunnel));
        w.up.seen()
            .last()
            .and_then(|s| s.authorization.clone())
            .as_deref()
            == Some("Token test-token-1")
    });
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_503_names_no_detail_and_the_log_does() {
    let mut w = World::new();
    let server = w.server(10.0);
    drop(w.ledger.take());
    let mut raw = TcpStream::connect(server.addr()).unwrap();
    write!(
        raw,
        "GET {PATH} HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Authorization: Bearer {}\r\n{TARGET}: {}\r\n\r\n",
        w.token,
        w.target()
    )
    .unwrap();
    let mut reply = String::new();
    raw.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 503"), "{reply}");
    assert!(
        reply.ends_with("\r\n\r\ntemporarily unavailable"),
        "{reply}"
    );
    let log = String::from_utf8(w.log.lock().unwrap().clone()).unwrap();
    assert!(log.contains("the controller is unavailable"), "{log}");
    server.drain(Duration::from_secs(5));
}

#[test]
fn only_a_plain_host_or_bracketed_v6_with_a_port_is_a_target() {
    for ok in ["h:443", "localhost:8443", "[::1]:443", "1.2.3.4:1"] {
        assert!(authority_ok(ok), "{ok}");
    }
    for bad in [
        "u@h:443", "a:b:443", "h:443/x", "h:99999", "h", ":443", "[::1:443", "[zz]:443",
    ] {
        assert!(!authority_ok(bad), "{bad}");
    }
}
