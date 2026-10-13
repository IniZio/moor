//! Relay behaviour against a local plain-HTTP fake upstream.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::proxy_gate::{Inject, Plan, RouteId};
use moor_core::Secret;

use super::chunked::ChunkedReader;
use super::Relay;
use crate::proxy::head::Head;
use crate::proxy::server::{Forward, ForwardHandler};
use crate::proxy::upstream::{CredError, RouteCredential, Upstreams};

/// Credential every fake lookup returns.
const SECRET: &str = "tok456";
/// Read deadline so a failing test turns red instead of hanging.
const WAIT: Duration = Duration::from_secs(10);

/// Returns [`SECRET`], or fails when `ok` is false.
struct FakeCreds {
    /// Whether lookups succeed.
    ok: bool,
}

impl RouteCredential for FakeCreds {
    fn credential(&self, _route: &RouteId) -> Result<Secret, CredError> {
        if self.ok {
            Ok(Secret::new(SECRET.as_bytes().to_vec()))
        } else {
            Err(CredError)
        }
    }
}

/// A request as the fake upstream saw it.
struct Seen {
    /// Request line.
    line: String,
    /// Lowercased header names with values, in arrival order.
    headers: Vec<(String, String)>,
    /// Decoded body.
    body: Vec<u8>,
}

/// Reads one request, decoding a chunked or length-delimited body.
fn read_request(stream: &mut TcpStream) -> Seen {
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (n, v) = h.split_once(':').unwrap();
        headers.push((n.to_ascii_lowercase(), v.trim().to_owned()));
    }
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if find("transfer-encoding").is_some_and(|v| v == "chunked") {
        ChunkedReader::new(&mut reader)
            .read_to_end(&mut body)
            .unwrap();
    } else if let Some(n) = find("content-length") {
        let n: u64 = n.parse().unwrap();
        reader.take(n).read_to_end(&mut body).unwrap();
    }
    Seen {
        line: line.trim_end().to_owned(),
        headers,
        body,
    }
}

/// Starts a fake upstream that serves one connection with `serve`.
fn upstream<T: Send + 'static>(
    serve: impl FnOnce(TcpStream) -> T + Send + 'static,
) -> (u16, JoinHandle<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || serve(listener.accept().unwrap().0));
    (port, handle)
}

/// Builds a relay whose `model` and `forge.git` routes point at `port`.
fn relay(port: u16, creds_ok: bool) -> Relay {
    let base = format!("http://127.0.0.1:{port}");
    let ups = Upstreams::new(
        [
            ("model".to_owned(), base.clone()),
            ("forge.git".to_owned(), base),
        ],
        true,
    )
    .unwrap();
    Relay::new(ups, Arc::new(FakeCreds { ok: creds_ok }))
}

/// Pairs a client socket with the proxy-side socket.
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    (client, server)
}

/// An admitted request for `route`.
fn forward(
    route: &str,
    method: &str,
    inject: Inject,
    headers: &[(&str, &str)],
    prefix: &[u8],
) -> Forward {
    let own = |h: &[(&str, &str)]| {
        h.iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect::<Vec<_>>()
    };
    let mut client_headers = own(headers);
    client_headers.push(("Authorization".to_owned(), "Bearer client-key".to_owned()));
    let framing = |(n, _): &&(String, String)| {
        n.eq_ignore_ascii_case("content-length") || n.eq_ignore_ascii_case("transfer-encoding")
    };
    let plan_headers = own(headers)
        .iter()
        .filter(|h| !framing(h))
        .cloned()
        .collect();
    Forward {
        head: Head {
            method: method.to_owned(),
            target: "/ignored".to_owned(),
            headers: client_headers,
        },
        plan: Plan {
            route: RouteId::new(route),
            method: method.to_owned(),
            upstream_path: "/v1/x?q=1".to_owned(),
            headers: plan_headers,
            inject,
        },
        body_prefix: prefix.to_vec(),
    }
}

/// Runs the relay on a thread; returns the client socket and the join handle.
fn spawn(relay: Relay, req: Forward) -> (TcpStream, JoinHandle<u16>) {
    let (client, mut server) = pair();
    let handle = thread::spawn(move || relay.forward(req, &mut server));
    (client, handle)
}

/// Reads the whole response until the relay closes the socket.
fn read_all(client: &mut TcpStream) -> String {
    let mut out = Vec::new();
    client.read_to_end(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// Responds `200` with a fixed body and closes.
fn ok_reply(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
        .unwrap();
}

/// Header value from a [`Seen`], by lowercase name.
fn header<'a>(seen: &'a Seen, name: &str) -> Option<&'a str> {
    seen.headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Plan headers plus the injected bearer are the only headers upstream sees.
#[test]
fn upstream_gets_plan_headers_and_injected_bearer_only() {
    let (port, up) = upstream(|mut s| {
        let seen = read_request(&mut s);
        ok_reply(&mut s);
        seen
    });
    let req = forward(
        "model",
        "POST",
        Inject::Bearer,
        &[
            ("content-type", "application/json"),
            ("content-length", "5"),
        ],
        b"hello",
    );
    let (mut client, done) = spawn(relay(port, true), req);
    let reply = read_all(&mut client);
    assert_eq!(done.join().unwrap(), 200);
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    assert!(reply.contains("connection: close\r\n"), "{reply}");
    assert!(reply.ends_with("\r\n\r\nok"), "{reply}");
    let seen = up.join().unwrap();
    assert_eq!(seen.line, "POST /v1/x?q=1 HTTP/1.1");
    assert_eq!(seen.body, b"hello");
    let mut names: Vec<&str> = seen.headers.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["authorization", "content-length", "content-type", "host"]
    );
    assert_eq!(header(&seen, "authorization"), Some("Bearer tok456"));
    assert_eq!(header(&seen, "content-length"), Some("5"));
}

/// Basic injection encodes `user:credential`.
#[test]
fn basic_injection_for_forge_git() {
    let (port, up) = upstream(|mut s| {
        let seen = read_request(&mut s);
        ok_reply(&mut s);
        seen
    });
    let req = forward(
        "forge.git",
        "GET",
        Inject::Basic {
            user: "x-access-token",
        },
        &[],
        b"",
    );
    let (mut client, done) = spawn(relay(port, true), req);
    read_all(&mut client);
    done.join().unwrap();
    let seen = up.join().unwrap();
    assert_eq!(
        header(&seen, "authorization"),
        Some("Basic eC1hY2Nlc3MtdG9rZW46dG9rNDU2")
    );
    assert!(seen.body.is_empty());
}

/// A chunked client body, split across prefix and socket, arrives intact.
#[test]
fn chunked_request_body_arrives_intact() {
    let (port, up) = upstream(|mut s| {
        let seen = read_request(&mut s);
        ok_reply(&mut s);
        seen
    });
    let big = vec![b'z'; 20_000];
    let req = forward(
        "model",
        "POST",
        Inject::Bearer,
        &[("transfer-encoding", "chunked")],
        b"5;ext=1\r\nhello\r\n",
    );
    let (mut client, done) = spawn(relay(port, true), req);
    client.write_all(b"6\r\n world\r\n").unwrap();
    client
        .write_all(format!("{:x}\r\n", big.len()).as_bytes())
        .unwrap();
    client.write_all(&big).unwrap();
    client.write_all(b"\r\n0\r\nx-trailer: 1\r\n\r\n").unwrap();
    let reply = read_all(&mut client);
    assert_eq!(done.join().unwrap(), 200, "{reply}");
    let seen = up.join().unwrap();
    let mut want = b"hello world".to_vec();
    want.extend_from_slice(&big);
    assert_eq!(seen.body, want);
    assert!(header(&seen, "content-length").is_none());
}

/// The first SSE event reaches the client before the upstream finishes.
#[test]
fn sse_events_stream_before_upstream_finishes() {
    let (go, wait) = mpsc::channel::<()>();
    let (port, up) = upstream(move |mut s| {
        read_request(&mut s);
        s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: one\n\n")
            .unwrap();
        s.flush().unwrap();
        wait.recv_timeout(WAIT).unwrap();
        s.write_all(b"data: two\n\n").unwrap();
    });
    let req = forward("model", "GET", Inject::Bearer, &[], b"");
    let (mut client, done) = spawn(relay(port, true), req);
    let mut seen = Vec::new();
    let mut buf = [0_u8; 256];
    while !String::from_utf8_lossy(&seen).contains("data: one\n\n") {
        let n = client.read(&mut buf).expect("first event never arrived");
        assert!(n > 0, "closed before first event");
        seen.extend_from_slice(buf.get(..n).unwrap());
    }
    go.send(()).unwrap();
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).unwrap();
    assert!(String::from_utf8(rest).unwrap().contains("data: two\n\n"));
    assert_eq!(done.join().unwrap(), 200);
    up.join().unwrap();
}

/// A redirect is relayed as is and never followed.
#[test]
fn redirect_passes_through_without_following() {
    let (port, up) = upstream(|mut s| {
        read_request(&mut s);
        s.write_all(
            b"HTTP/1.1 302 Found\r\nlocation: http://elsewhere.invalid/x\r\ncontent-length: 0\r\n\r\n",
        )
        .unwrap();
        s.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut extra = Vec::new();
        let _ = s.read_to_end(&mut extra);
        extra
    });
    let req = forward("model", "GET", Inject::Bearer, &[], b"");
    let (mut client, done) = spawn(relay(port, true), req);
    let reply = read_all(&mut client);
    assert_eq!(done.join().unwrap(), 302);
    assert!(reply.starts_with("HTTP/1.1 302 Found\r\n"), "{reply}");
    assert!(
        reply.contains("location: http://elsewhere.invalid/x\r\n"),
        "{reply}"
    );
    assert!(up.join().unwrap().is_empty(), "second request was sent");
}

/// A refused connection yields a fixed 502.
#[test]
fn unreachable_upstream_is_502() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let req = forward("model", "GET", Inject::Bearer, &[], b"");
    let (mut client, done) = spawn(relay(port, true), req);
    let reply = read_all(&mut client);
    assert_eq!(done.join().unwrap(), 502);
    assert!(reply.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{reply}");
    assert!(
        reply.ends_with("\r\n\r\nupstream unreachable for route model\n"),
        "{reply}"
    );
}

/// A credential failure yields a 502 naming only the route.
#[test]
fn credential_error_is_502_without_secret() {
    let req = forward("model", "GET", Inject::Bearer, &[], b"");
    let (mut client, done) = spawn(relay(1, false), req);
    let reply = read_all(&mut client);
    assert_eq!(done.join().unwrap(), 502);
    assert!(
        reply.ends_with("\r\n\r\nupstream credential unavailable for route model\n"),
        "{reply}"
    );
    assert!(!reply.contains(SECRET) && !reply.contains("client-key"));
}

/// Framing errors in a chunked body are rejected, not passed on.
#[test]
fn chunked_decoder_rejects_bad_framing() {
    for bad in ["zz\r\nabc\r\n0\r\n\r\n", "3\r\nabcXX0\r\n\r\n", "3\r\nab"] {
        let mut out = Vec::new();
        let r = ChunkedReader::new(bad.as_bytes()).read_to_end(&mut out);
        assert!(r.is_err(), "{bad:?} accepted");
    }
    let mut out = Vec::new();
    ChunkedReader::new(&b"3;a=b\r\nabc\r\n0\r\nt: 1\r\n\r\n"[..])
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out, b"abc");
}
