//! Per-connection handling: tunnel setup, request loop and forwarding.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use rustls::{ServerConnection, StreamOwned};

use super::body::send_rest;
use super::body::BodyState;
use super::check::{unencoded, Relay};
use super::gate::{gate, without_credential, Gate};
use super::git;
use super::http::{copy_body, read_head, request_framing, write_refusal, Framing, Head, HttpError};
use super::relay::relay;
use super::repo_ids::{gate_id, IdGate};
use super::route::{classify, Route};
use super::tunnel::{dial, parse_authority, splice};
use super::upstream::{TlsStream, Upstream};
use super::{Shared, EGRESS_RULE, GUARD_CATEGORY, SCOPE_CATEGORY};
use crate::proxy::chain;
use crate::proxy::guard::Route as Path;

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
pub(super) type Client = BufReader<StreamOwned<ServerConnection, TcpStream>>;
/// The upstream a connection forwards to, opened lazily.
pub(super) struct Up<'a> {
    /// The host it dials.
    to: &'a Upstream,
    /// The open connection, if any.
    conn: Option<BufReader<TlsStream>>,
}

/// What follows the request head on the wire.
pub(super) enum Payload<'a> {
    /// Bytes already read, sent as they came.
    Raw(&'a [u8]),
    /// A body still to be streamed from the client.
    Stream(Framing),
    /// A push's command list, already read, then the rest of its body from the client.
    Pushed(&'a [u8], BodyState),
}

/// Whether to keep serving the connection.
#[derive(PartialEq, Eq)]
pub(super) enum Flow {
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
    let Ok(mode) = tunnel(&mut tcp, shared) else {
        return;
    };
    let Ok(conn) = ServerConnection::new(shared.tls.clone()) else {
        return;
    };
    let mut client: Client = BufReader::new(StreamOwned::new(conn, tcp));
    let (one, to): (fn(&Shared, &mut Client, &mut Up<'_>) -> Flow, _) = match (mode, &shared.git) {
        (Mode::Git, Some(g)) => (git::serve_one, &g.upstream),
        _ => (serve_one, &shared.upstream),
    };
    let mut up = Up { to, conn: None };
    while one(shared, &mut client, &mut up) == Flow::Continue {}
    client.get_mut().conn.send_close_notify();
    drop(client.get_mut().flush());
}

/// Which host a tunnel the filter terminates is for.
enum Mode {
    /// The judged API host.
    Api,
    /// The git host, whose pushes the push guard reads.
    Git,
}

/// Handles the `CONNECT`. The judged host and port get the GraphQL path and the git host the
/// push guard; any other target is tunnelled raw, and only to an address the guard vetted.
fn tunnel(tcp: &mut TcpStream, shared: &Shared) -> Result<Mode, ()> {
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
        return scope_refusal(tcp, shared, &head).map(|()| Mode::Api);
    };
    let git = shared.git.as_ref().map(|g| g.host.as_str());
    let mode = if git.is_some_and(|g| host.eq_ignore_ascii_case(g)) && port == shared.port {
        Mode::Git
    } else if !host.eq_ignore_ascii_case(&shared.host) {
        return raw_tunnel(tcp, shared, &host, port, &pending).map(|()| Mode::Api);
    } else if port != shared.port {
        return scope_refusal(tcp, shared, &head).map(|()| Mode::Api);
    } else {
        Mode::Api
    };
    tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(drop)?;
    Ok(mode)
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
    let Ok(route) = shared.guard.route(host, port) else {
        drop(write_refusal(tcp, 403, "moor: destination not allowed"));
        drain(tcp);
        shared.refuse_host(host, EGRESS_RULE, "CONNECT", GUARD_CATEGORY);
        return Err(());
    };
    let dialled = match route {
        Path::Direct(addrs) => dial(&addrs).ok_or_else(|| "moor: upstream unavailable".to_owned()),
        Path::Proxy(proxy) => chain::connect(proxy, host, port).map_err(|e| format!("moor: {e}")),
    };
    let upstream = match dialled {
        Ok(upstream) => upstream,
        Err(msg) => {
            drop(write_refusal(tcp, 502, &msg));
            drain(tcp);
            return Err(());
        }
    };
    tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(drop)?;
    splice(tcp, &upstream, pending);
    Err(())
}

/// Replies with a refusal and ends the connection.
fn refuse(shared: &Shared, client: &mut Client, method: &str, status: u16, msg: &str) -> Flow {
    if matches!(status, 401 | 403) {
        shared.excuse(status);
    }
    deliver_refusal(client, status, msg);
    shared.refuse(method, SCOPE_CATEGORY);
    Flow::Close
}

/// Reads one request head, or answers why not and says to close.
pub(super) fn request_head(client: &mut Client) -> Result<Head, Flow> {
    let head = match read_head(client) {
        Ok(h) => h,
        Err(HttpError::Malformed) => return Err(bad(client, 400, "moor: malformed request")),
        Err(HttpError::TooLarge) => return Err(bad(client, 413, "moor: request head too large")),
        Err(_) => return Err(Flow::Close),
    };
    if !head.target.starts_with('/') {
        return Err(bad(client, 400, "moor: malformed request"));
    }
    // nono matches the whole target, `#` included, while the routes below judge only the
    // part before it; a fragment never belongs on the wire, so nothing is forwarded.
    if head.target.contains('#') {
        return Err(bad(client, 400, "moor: fragment in request target"));
    }
    Ok(head)
}

/// Forwards a request whose body is not read, streaming it to `to`.
pub(super) fn pass(shared: &Shared, client: &mut Client, up: &mut Up<'_>, head: &Head) -> Flow {
    match request_framing(head) {
        Ok(f) => forward(shared, client, up, head, &Payload::Stream(f), Relay::Plain),
        Err(_) => bad(client, 400, "moor: malformed request"),
    }
}

/// Reads and handles one request to the judged host.
fn serve_one(shared: &Shared, client: &mut Client, up: &mut Up<'_>) -> Flow {
    let head = match request_head(client) {
        Ok(h) => h,
        Err(flow) => return flow,
    };
    match classify(&head.target) {
        Route::Passthrough => pass(shared, client, up, &head),
        Route::RepoId(canonical) => repo_id(shared, client, up, &head, &canonical),
        Route::Graphql => graphql(shared, client, up, &head),
    }
}

/// Handles a request to the GraphQL path.
fn graphql(shared: &Shared, client: &mut Client, up: &mut Up<'_>, head: &Head) -> Flow {
    if head.target != "/graphql" {
        let reason = "query string or non-canonical path on /graphql is not allowed";
        return refuse(
            shared,
            client,
            &head.method,
            403,
            &format!("moor: {reason}"),
        );
    }
    match gate(shared, head, client) {
        Gate::Forward(body, linked) if linked.is_empty() => {
            forward(shared, client, up, head, &Payload::Raw(&body), Relay::Plain)
        }
        Gate::Forward(body, linked) => {
            let _turn = shared.checking.take();
            let asked = unencoded(head);
            let relay = Relay::Checked(&linked);
            forward(shared, client, up, &asked, &Payload::Raw(&body), relay)
        }
        Gate::Anonymous(body, reason, query) => {
            // The one place a request leaves without its credential. GitHub
            // answers the placeholder with 401, which clients read as a bad
            // token. No `Authorization` gets 403, which they read as a rate limit.
            shared.withhold(&head.method, "/graphql", &reason, query);
            let bare = without_credential(
                head,
                &shared.credential_headers,
                shared.placeholder.as_deref(),
            );
            forward(
                shared,
                client,
                up,
                &bare,
                &Payload::Raw(&body),
                Relay::Anonymous,
            )
        }
        Gate::Refuse(s, m) => refuse(shared, client, &head.method, s, &m),
        Gate::Abort => Flow::Close,
    }
}

/// Handles a request on the numeric repo-id form.
fn repo_id(
    shared: &Shared,
    client: &mut Client,
    up: &mut Up<'_>,
    head: &Head,
    canonical: &str,
) -> Flow {
    let Ok(f) = request_framing(head) else {
        return bad(client, 400, "moor: malformed request");
    };
    match gate_id(shared, head, canonical) {
        IdGate::Unchanged | IdGate::Forward => {
            forward(shared, client, up, head, &Payload::Stream(f), Relay::Plain)
        }
        IdGate::Anonymous(reason) => {
            shared.withhold(&head.method, &head.target, reason, None);
            let bare = without_credential(
                head,
                &shared.credential_headers,
                shared.placeholder.as_deref(),
            );
            forward(
                shared,
                client,
                up,
                &bare,
                &Payload::Stream(f),
                Relay::Anonymous,
            )
        }
    }
}

/// Answers a request that is not acceptable HTTP, without recording it.
pub(super) fn bad(client: &mut Client, status: u16, msg: &str) -> Flow {
    deliver_refusal(client, status, msg);
    Flow::Close
}

/// Writes a refusal, closes TLS cleanly and discards the unread request so
/// the answer is not lost to a connection reset.
pub(super) fn deliver_refusal(client: &mut Client, status: u16, msg: &str) {
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

/// Sends the request upstream and relays the response.
pub(super) fn forward(
    shared: &Shared,
    client: &mut Client,
    up: &mut Up<'_>,
    head: &Head,
    payload: &Payload<'_>,
    how: Relay<'_>,
) -> Flow {
    if up.conn.is_none() {
        up.conn = up.to.connect().ok().map(BufReader::new);
    }
    let Some(conn) = up.conn.as_mut() else {
        return unavailable(client);
    };
    if send(conn, client, head, payload).is_err() {
        up.conn = None;
        return unavailable(client);
    }
    let mut sent = false;
    let flow = relay(shared, conn, client, head, &mut sent, how);
    if flow == Flow::Close {
        up.conn = None;
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
        Payload::Pushed(prefix, rest) => send_rest(client, w, prefix, *rest).map_err(drop)?,
    }
    w.flush().map_err(drop)
}
