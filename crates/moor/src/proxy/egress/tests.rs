//! The shared egress Proxy end to end on loopback: a WebSocket per CONNECT, keyed by the Box
//! token, through the Box's proxy to a fake HTTPS upstream, with the ledger served as the
//! controller serves it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
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

use super::{Config, Server, PATH, TARGET};
use crate::proxy::ledger::tests::{serve_on, Served, EGRESS_KEY};
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{put_secret, test_vault_key};
use crate::testing::rng::FakeRng;
use crate::vault::Vault;

/// The Box's token.
const TOKEN: &str = "box-token-1";
/// The credential the Box user holds, which only the upstream may see.
const SECRET: &str = "test-token-1";

/// A controller's store and ledger, an upstream, a Box with a token, and the shared Proxy.
struct World {
    dir: tempfile::TempDir,
    store: Store,
    up: Upstream,
    id: BoxId,
    _ledger: Served,
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
        store.set_egress_token(id, TOKEN.as_bytes()).unwrap();
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
            _ledger: ledger,
        }
    }

    /// The shared Proxy, letting each token open `burst` tunnels and no more.
    fn server(&self, burst: f64) -> Server {
        Server::start(Config {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            ledger: self.dir.path().join("proxy.sock"),
            key: Zeroizing::new(EGRESS_KEY.to_owned()),
            work_dir: self.dir.path().join("egress"),
            rate: 0.0,
            burst,
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
    let tunnel = open(&server, TOKEN, &w.target()).unwrap();
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
    assert_eq!(open(&server, TOKEN, "no-port").err(), Some(400));
    assert!(w.up.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn the_token_of_a_closed_box_is_refused() {
    let w = World::new();
    let server = w.server(10.0);
    assert!(open(&server, TOKEN, &w.target()).is_ok());
    w.close_box();
    assert_eq!(open(&server, TOKEN, &w.target()).err(), Some(401));
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_token_past_its_rate_is_refused() {
    let w = World::new();
    let server = w.server(2.0);
    assert!(open(&server, TOKEN, &w.target()).is_ok());
    assert!(open(&server, TOKEN, &w.target()).is_ok());
    assert_eq!(open(&server, TOKEN, &w.target()).err(), Some(429));
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
    let tunnel = open(&server, TOKEN, &w.target()).unwrap();
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
