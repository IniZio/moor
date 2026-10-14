//! Forge requests driven through the listener, gate, resolver and relay to a
//! local plain-HTTP upstream.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::proxy_gate::{Inject, RouteRow, RouteTable};
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{Secret, TaskId};

use crate::adapters::clock::SystemClock;
use crate::env::Env;
use crate::proxy::resolver::{ModelToken, Resolver};
use crate::proxy::server::{body_cap, ProxyOpts, HEAD_BUDGET};
use crate::proxy::upstream::{auth_value, Upstreams};
use crate::proxy::{Proxy, ProxyLog};
use crate::testing::clock::FakeClock;
use crate::testing::forge::FakeForge;

const NONCE: &str = "q3Jt0bXo1n9Zr2Kc8vWm4yHs7aLp6eTg5uDf0iBk-_A";
const TOKEN: &[u8] = b"ghs_forge_e2e_token";
/// Read deadline so a failing test turns red instead of hanging.
const WAIT: Duration = Duration::from_secs(10);

/// What the upstream saw of one request.
struct Seen {
    /// Request line.
    line: String,
    /// Value of the `authorization` header.
    auth: String,
}

/// Serves `count` connections, answering 200, and reports each request.
fn upstream(count: usize) -> (u16, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..count {
            let mut stream = listener.accept().unwrap().0;
            stream.set_read_timeout(Some(WAIT)).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut auth = String::new();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                if let Some((n, v)) = h.split_once(':') {
                    if n.eq_ignore_ascii_case("authorization") {
                        v.trim().clone_into(&mut auth);
                    }
                }
            }
            let reply = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";
            stream.write_all(reply.as_bytes()).unwrap();
            tx.send(Seen {
                line: line.trim_end().to_owned(),
                auth,
            })
            .unwrap();
        }
    });
    (port, rx)
}

/// Sends `GET path` as the Box would and returns the whole reply.
fn get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    drop(s.read_to_end(&mut out));
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `seen` is exactly the header value `inject` produces for [`TOKEN`].
fn injected(seen: &str, inject: Inject) -> bool {
    auth_value(inject, &Secret::new(TOKEN.to_vec())).is_some_and(|v| v.expose() == seen.as_bytes())
}

/// Starts the real listener, gate, resolver and relay against the upstream at `port`.
fn start(port: u16) -> Proxy {
    let base = format!("http://127.0.0.1:{port}");
    let env = Env::from_pairs([
        ("MOOR_TEST", "1"),
        ("MOOR_PROXY_UPSTREAM_FORGE_GIT", base.as_str()),
        ("MOOR_PROXY_UPSTREAM_FORGE_API", base.as_str()),
    ]);
    let upstreams = Upstreams::from_env(&env).unwrap();
    let repo = ForgeRepo::new("IniZio", "x");
    let forge = Arc::new(FakeForge::new().with_token(repo.clone(), TOKEN));
    let clock = Arc::new(SystemClock::real());
    let model: ModelToken = Arc::new(|| Err("unused".to_owned()));
    let resolver = Resolver::new(model, forge, Some(repo), clock.clone());
    let relay = Arc::new(crate::proxy::relay::Relay::new(
        upstreams,
        Arc::new(resolver),
        clock.clone(),
        body_cap,
    ));
    let dir = tempfile::tempdir().unwrap();
    let log_clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let log = ProxyLog::open(dir.path(), TaskId::new(1), log_clock).unwrap();
    let table = RouteTable::new(vec![
        RouteRow::forge_git("/github"),
        RouteRow::forge_api("/github-api"),
    ]);
    let opts = ProxyOpts {
        max_conns: 4,
        task_repo: Some("IniZio/x".to_owned()),
        clock,
        head_budget: HEAD_BUDGET,
        body_cap,
    };
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    Proxy::start(table, nonce, log, relay, opts).unwrap()
}

#[test]
fn forge_requests_reach_the_upstream_without_the_route_prefix() {
    let (port, seen) = upstream(2);
    let proxy = start(port);

    let git = get(
        proxy.port(),
        &format!("/t/{NONCE}/github/IniZio/x.git/info/refs?service=git-upload-pack"),
    );
    let api = get(
        proxy.port(),
        &format!("/t/{NONCE}/github-api/repos/IniZio/x/pulls"),
    );
    proxy.shutdown();

    let first = seen.recv_timeout(WAIT).unwrap();
    let second = seen.recv_timeout(WAIT).unwrap();
    assert_eq!(
        first.line,
        "GET /IniZio/x.git/info/refs?service=git-upload-pack HTTP/1.1"
    );
    assert!(first.auth.starts_with("Basic "));
    assert!(injected(
        &first.auth,
        Inject::Basic {
            user: "x-access-token"
        }
    ));
    assert_eq!(second.line, "GET /repos/IniZio/x/pulls HTTP/1.1");
    assert!(second.auth.starts_with("Bearer "));
    assert!(injected(&second.auth, Inject::Bearer));
    assert!(git.starts_with("HTTP/1.1 200"), "{git}");
    assert!(api.starts_with("HTTP/1.1 200"), "{api}");
}
