//! Per-connection handling: tunnel setup, request loop and forwarding.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use rustls::{ServerConnection, StreamOwned};

use super::gate::{gate, without_credential, Gate};
use super::http::{
    copy_body, read_head, read_response_head, request_framing, response_framing, wants_close,
    write_refusal, Framing, Head, HttpError,
};
use super::tunnel::{dial, parse_authority, splice};
use super::upstream::TlsStream;
use super::{Shared, EGRESS_RULE, GUARD_CATEGORY, SCOPE_CATEGORY};

/// Read and write timeout on the client socket.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Size of one discarding read.
const DRAIN_CHUNK: usize = 1 << 12;
/// Most reads while discarding after a refusal: 1 MiB in all. Discarding
/// also ends at the first idle gap or close.
const DRAIN_READS: usize = 256;
/// Read timeout while discarding.
const DRAIN_STEP: Duration = Duration::from_millis(200);

/// The client side after the handshake.
type Client = BufReader<StreamOwned<ServerConnection, TcpStream>>;
/// The lazily opened upstream side.
type Up = Option<BufReader<TlsStream>>;

/// What follows the request head on the wire.
enum Payload<'a> {
    /// Bytes already read, sent as they came.
    Raw(&'a [u8]),
    /// A body still to be streamed from the client.
    Stream(Framing),
}

/// Whether to keep serving the connection.
#[derive(PartialEq, Eq)]
enum Flow {
    /// Read the next request.
    Continue,
    /// Close the connection.
    Close,
}

/// Serves one accepted connection; every failure just ends it.
pub(super) fn connection(mut tcp: TcpStream, shared: &Shared) {
    if tcp.set_read_timeout(Some(TIMEOUT)).is_err() || tcp.set_write_timeout(Some(TIMEOUT)).is_err()
    {
        return;
    }
    if tunnel(&mut tcp, shared).is_err() {
        return;
    }
    let Ok(conn) = ServerConnection::new(shared.tls.clone()) else {
        return;
    };
    let mut client: Client = BufReader::new(StreamOwned::new(conn, tcp));
    let mut up: Up = None;
    while serve_one(shared, &mut client, &mut up) == Flow::Continue {}
    client.get_mut().conn.send_close_notify();
    drop(client.get_mut().flush());
}

/// Handles the `CONNECT`. The judged host and port get the GraphQL path;
/// any other target is tunnelled raw, and only to an address the guard
/// vetted.
fn tunnel(tcp: &mut TcpStream, shared: &Shared) -> Result<(), ()> {
    let (head, pending) = {
        let mut r = BufReader::new(&*tcp);
        let head = read_head(&mut r).map_err(drop)?;
        (head, r.buffer().to_vec())
    };
    let target = if head.method == "CONNECT" {
        parse_authority(&head.target)
    } else {
        None
    };
    let Some((host, port)) = target else {
        return scope_refusal(tcp, shared, &head);
    };
    if !host.eq_ignore_ascii_case(&shared.host) {
        return raw_tunnel(tcp, shared, &host, port, &pending);
    }
    if port != shared.port {
        return scope_refusal(tcp, shared, &head);
    }
    tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(drop)?;
    Ok(())
}

/// Refuses a tunnel to a port other than the judged one.
fn scope_refusal(tcp: &mut TcpStream, shared: &Shared, head: &Head) -> Result<(), ()> {
    drop(write_refusal(tcp, 403, "moor: tunnel target not allowed"));
    drain(tcp);
    shared.refuse(&head.method, SCOPE_CATEGORY);
    Err(())
}

/// Resolves through the guard and splices to the vetted address. Always
/// ends the connection afterwards: the tunnel is not served by the filter.
fn raw_tunnel(
    tcp: &mut TcpStream,
    shared: &Shared,
    host: &str,
    port: u16,
    pending: &[u8],
) -> Result<(), ()> {
    let Ok(addrs) = shared.guard.resolve(host, port) else {
        drop(write_refusal(tcp, 403, "moor: destination not allowed"));
        drain(tcp);
        shared.refuse_host(host, EGRESS_RULE, "CONNECT", GUARD_CATEGORY);
        return Err(());
    };
    let Some(upstream) = dial(&addrs) else {
        drop(write_refusal(tcp, 502, "moor: upstream unavailable"));
        drain(tcp);
        return Err(());
    };
    tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(drop)?;
    splice(tcp, &upstream, pending);
    Err(())
}

/// Replies with a refusal and ends the connection.
fn refuse(shared: &Shared, client: &mut Client, method: &str, status: u16, msg: &str) -> Flow {
    deliver_refusal(client, status, msg);
    shared.refuse(method, SCOPE_CATEGORY);
    Flow::Close
}

/// Reads and handles one request.
fn serve_one(shared: &Shared, client: &mut Client, up: &mut Up) -> Flow {
    let head = match read_head(client) {
        Ok(h) => h,
        Err(HttpError::Malformed) => return bad(client, 400, "moor: malformed request"),
        Err(HttpError::TooLarge) => return bad(client, 413, "moor: request head too large"),
        Err(_) => return Flow::Close,
    };
    if !head.target.starts_with('/') {
        return bad(client, 400, "moor: malformed request");
    }
    match classify(&head.target) {
        Route::Passthrough => match request_framing(&head) {
            Ok(f) => forward(shared, client, up, &head, &Payload::Stream(f), false),
            Err(_) => bad(client, 400, "moor: malformed request"),
        },
        Route::Graphql if head.target != "/graphql" => {
            let reason = "query string or non-canonical path on /graphql is not allowed";
            refuse(
                shared,
                client,
                &head.method,
                403,
                &format!("moor: {reason}"),
            )
        }
        Route::Graphql => match gate(shared, &head, client) {
            Gate::Forward(body) => forward(shared, client, up, &head, &Payload::Raw(&body), false),
            Gate::Anonymous(body) => {
                // The one place a request leaves without its credential.
                shared.withhold(&head.method);
                let bare = without_credential(&head, &shared.credential_headers);
                forward(shared, client, up, &bare, &Payload::Raw(&body), true)
            }
            Gate::Refuse(s, m) => refuse(shared, client, &head.method, s, &m),
            Gate::Abort => Flow::Close,
        },
    }
}

/// Answers a request that is not acceptable HTTP, without recording it.
fn bad(client: &mut Client, status: u16, msg: &str) -> Flow {
    deliver_refusal(client, status, msg);
    Flow::Close
}

/// Writes a refusal, closes TLS cleanly and discards the unread request so
/// the answer is not lost to a connection reset.
fn deliver_refusal(client: &mut Client, status: u16, msg: &str) {
    let stream = client.get_mut();
    drop(write_refusal(stream, status, msg));
    stream.conn.send_close_notify();
    drop(stream.flush());
    drain(&mut stream.sock);
}

/// Half-closes `tcp`, then reads and drops what the peer sends, within a
/// byte bound. The bytes are never kept or forwarded.
fn drain(tcp: &mut TcpStream) {
    drop(tcp.shutdown(Shutdown::Write));
    if tcp.set_read_timeout(Some(DRAIN_STEP)).is_err() {
        return;
    }
    let mut buf = [0u8; DRAIN_CHUNK];
    for _ in 0..DRAIN_READS {
        if matches!(tcp.read(&mut buf), Ok(0) | Err(_)) {
            break;
        }
    }
}

/// Which handling a request path gets.
enum Route {
    /// Forwarded without looking at the body.
    Passthrough,
    /// The GraphQL endpoint, however it is spelled.
    Graphql,
}

/// Decides the route from the path alone.
fn classify(target: &str) -> Route {
    if canonical_path(target) == "/graphql" {
        Route::Graphql
    } else {
        Route::Passthrough
    }
}

/// The path part of `target` percent-decoded, lowercased, with backslashes
/// as slashes, dot segments resolved and empty segments dropped.
fn canonical_path(target: &str) -> String {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let (b'%', Some(v)) = (b, hex) {
            decoded.push(v);
            i += 3;
        } else {
            decoded.push(b);
            i += 1;
        }
    }
    let text = String::from_utf8_lossy(&decoded)
        .to_ascii_lowercase()
        .replace('\\', "/");
    let mut segments: Vec<&str> = Vec::new();
    for seg in text.split('/') {
        match seg {
            "" | "." => {}
            ".." => drop(segments.pop()),
            s => segments.push(s),
        }
    }
    format!("/{}", segments.join("/"))
}

/// Sends the request upstream and relays the response.
fn forward(
    shared: &Shared,
    client: &mut Client,
    up: &mut Up,
    head: &Head,
    payload: &Payload<'_>,
    anonymous: bool,
) -> Flow {
    if up.is_none() {
        *up = shared.upstream.connect().ok().map(BufReader::new);
    }
    let Some(conn) = up.as_mut() else {
        return unavailable(client);
    };
    if send(conn, client, head, payload).is_err() {
        *up = None;
        return unavailable(client);
    }
    let mut sent = false;
    let flow = relay(shared, conn, client, head, &mut sent, anonymous);
    if flow == Flow::Close {
        *up = None;
    }
    if !sent && flow == Flow::Close {
        return unavailable(client);
    }
    flow
}

/// Replies 502 and ends the connection.
fn unavailable(client: &mut Client) -> Flow {
    bad(client, 502, "moor: upstream unavailable")
}

/// Writes the request head and payload upstream.
fn send(
    conn: &mut BufReader<TlsStream>,
    client: &mut Client,
    head: &Head,
    payload: &Payload<'_>,
) -> Result<(), ()> {
    let w = conn.get_mut();
    w.write_all(&head.raw).map_err(drop)?;
    match payload {
        Payload::Raw(body) => w.write_all(body).map_err(drop)?,
        Payload::Stream(f) => copy_body(client, w, f).map_err(drop)?,
    }
    w.flush().map_err(drop)
}

/// Relays the response, setting `sent` once a byte reached the client. A
/// rejection is noted before any byte is relayed.
fn relay(
    shared: &Shared,
    conn: &mut BufReader<TlsStream>,
    client: &mut Client,
    head: &Head,
    sent: &mut bool,
    anonymous: bool,
) -> Flow {
    let resp = loop {
        let Ok(resp) = read_response_head(conn) else {
            return Flow::Close;
        };
        if (100..200).contains(&resp.status) && resp.status != 101 {
            if client.get_mut().write_all(&resp.raw).is_err() || client.get_mut().flush().is_err() {
                *sent = true;
                return Flow::Close;
            }
            *sent = true;
            continue;
        }
        break resp;
    };
    match (anonymous, resp.status) {
        (false, 401) => (shared.on_credentialed_401)(head),
        (true, 401 | 403) => shared.reject_anonymous(resp.status),
        _ => {}
    }
    *sent = true;
    let Ok(framing) = response_framing(&resp, &head.method) else {
        return Flow::Close;
    };
    let out = client.get_mut();
    if out.write_all(&resp.raw).is_err() || copy_body(conn, out, &framing).is_err() {
        return Flow::Close;
    }
    if out.flush().is_err()
        || resp.status == 101
        || framing == Framing::UntilClose
        || wants_close(&head.headers)
        || wants_close(&resp.headers)
    {
        return Flow::Close;
    }
    Flow::Continue
}
