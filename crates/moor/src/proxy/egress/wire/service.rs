//! A tunnel to a host the Proxy serves: one plain request, stripped of the `X-Moor-*` headers the
//! Box sent and stamped with the Proxy's own, sent to the service's upstream. The upstream's name
//! is looked up on each connection and dialled only when every address it has is loopback or
//! private, so no config or DNS answer can send a Box's identity off to a public address.
//!
//! A request for the machine service goes to the runner that holds the Box's machine's link when
//! the ledger names one. A holder that cannot be dialled, or that answers 421 because the link
//! moved, costs one more read of the ledger and one more try before the Box sees an error.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use moor_core::config::{reachable, Served};
use tokio::io::AsyncWriteExt as _;
use tokio::net::{lookup_host, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::super::boxes::Route;
use super::super::ServedRoute;
use super::{
    answer, authority_ok, read_head, refuse, relay, upgrade, ws_head, Open, Peer, HEAD_TIMEOUT,
    MAX_MESSAGE,
};
use crate::proxy::front::{hold_headers, hold_or_missing, plain_request, Hold};

#[cfg(test)]
mod tests;

/// How long a served host's upstream gets to be looked up, and to accept a connection, before
/// the tunnel is a 502.
const MACHINE_DIAL: Duration = Duration::from_secs(3);
/// How long a runner may take no byte of a request before the Box gets a 502. The clock runs only
/// while a write waits on the runner, so a Box's slow upload never starts it. A live runner
/// drains its socket as soon as it is scheduled, and gets as long as it has to answer.
const WRITE_STALL: Duration = HEAD_TIMEOUT;
/// Asks the runner to answer 421 when it does not hold the Box's machine's link, and to leave
/// the request undone, so it can go to the new holder.
const HOLDER_ONLY: &str = "X-Moor-Holder-Only";
/// What the runner answers to [`HOLDER_ONLY`] when it holds no link to the machine.
const MISDIRECTED: u16 = 421;
/// Why the Box gets a 502 when no runner could be reached for its request.
const NOT_ANSWERING: &str = "the machine service is not answering";
/// Why the Box gets a 502 when a served host's upstream does not answer.
const DOWN: &str = "the service is not answering";
/// Why the Box gets a 502 when a served host's upstream names an address off this host's network.
const PUBLIC: &str = "the service's upstream is not a loopback or private address";
/// Why the Box gets a 502 when the runner went away before answering; the call may have run,
/// so it is not sent again.
const LOST: &str = "the machine runner went away before answering; the call may have run";

/// A served host, reached for one request.
struct Reached {
    /// The connection.
    stream: TcpStream,
    /// The address dialled.
    addr: String,
    /// The `X-Moor-*` headers the request carries: for the machine service, what the Box held
    /// when the ledger was asked.
    own: Option<Vec<(&'static str, String)>>,
    /// Whether it is the holder the ledger named on its first answer, which may still be
    /// asked again.
    holder: bool,
}

/// What the holder made of a request marked [`HOLDER_ONLY`].
enum Tried {
    /// It answered; the answer's head and what came after it.
    Answered(Vec<u8>),
    /// It holds no link to the machine and left the request undone.
    Elsewhere,
    /// The connection failed, so the request may have run.
    Lost,
}

/// The Box's plain request, as read from the tunnel.
struct Plain {
    /// Its head, as the Box sent it.
    head: Vec<u8>,
    /// Its head in origin form, marked with what the Box holds.
    request: Vec<u8>,
    /// Its body as read so far.
    body: Vec<u8>,
}

/// Dials the served host for the Box of `ask`, upgrades the connection and relays the plain
/// request and its answer.
pub(super) async fn tunnel<S: Peer>(
    mut stream: S,
    service: &ServedRoute,
    ask: &Open,
    route: &Route,
) {
    let reached = match reach(service, route).await {
        Ok(reached) => reached,
        Err(why) => return answer(&mut stream, 502, why).await,
    };
    let Some(mut ws) = upgrade(stream, &ask.key).await else {
        return;
    };
    let Some((head, body)) = ws_head(&mut ws).await else {
        return;
    };
    let Some(request) = marked(
        &head,
        &ask.target,
        reached.own.as_deref(),
        service.key.as_deref(),
    ) else {
        return refuse(&mut ws).await;
    };
    let plain = Plain {
        head,
        request,
        body,
    };
    deliver(ws, reached, plain, service, (ask, route)).await;
}

/// Sends `plain` to what `reached` dialled and relays the answer; a holder that no longer holds
/// the link hands it to the one the ledger names now, when the request can go twice.
async fn deliver<S: Peer>(
    mut ws: WebSocketStream<S>,
    reached: Reached,
    mut plain: Plain,
    service: &ServedRoute,
    asked: (&Open, &Route),
) {
    let mut upstream = reached.stream;
    if reached.holder && whole_body(&mut ws, &plain.request, &mut plain.body).await {
        match hand_off(&mut upstream, &mut plain, service, asked, &reached.addr).await {
            Ok(Some(rest)) => return relay(ws, upstream, rest).await,
            Ok(None) => {}
            Err(why) => return gateway_error(&mut ws, why).await,
        }
    }
    if !send(&mut upstream, &plain.request, &plain.body).await {
        return gateway_error(&mut ws, NOT_ANSWERING).await;
    }
    relay(ws, upstream, Vec::new()).await;
}

/// Tries `plain` on the holder at `failed` over `upstream`: its answer to relay, or `None` once
/// `upstream` and `plain` are moved to where the ledger points now; why the Box gets a 502.
async fn hand_off(
    upstream: &mut TcpStream,
    plain: &mut Plain,
    service: &ServedRoute,
    asked: (&Open, &Route),
    failed: &str,
) -> Result<Option<Vec<u8>>, &'static str> {
    match try_holder(upstream, &plain.request, &plain.body).await {
        Tried::Answered(rest) => return Ok(Some(rest)),
        Tried::Lost => return Err(LOST),
        Tried::Elsewhere => {}
    }
    let (stream, request) = elsewhere(service, asked, failed, &plain.head)
        .await
        .ok_or(NOT_ANSWERING)?;
    (*upstream, plain.request) = (stream, request);
    Ok(None)
}

/// After the holder at `failed` said it holds no link: what the ledger names now, dialled, and
/// `head` marked with what the Box holds now.
async fn elsewhere(
    service: &ServedRoute,
    asked: (&Open, &Route),
    failed: &str,
    head: &[u8],
) -> Option<(TcpStream, Vec<u8>)> {
    let (ask, route) = asked;
    let again = reask(service, route, failed).await.ok()?;
    let request = marked(
        head,
        &ask.target,
        again.own.as_deref(),
        service.key.as_deref(),
    )?;
    Some((again.stream, request))
}

/// The plain request `head` for `target` in origin form, marked with `own` and `key`.
fn marked(
    head: &[u8],
    target: &str,
    own: Option<&[(&'static str, String)]>,
    key: Option<&str>,
) -> Option<Vec<u8>> {
    let request = plain_request(head, target, Some(own?))?;
    Some(match key {
        Some(key) => with_header(&request, "X-Moor-Service-Key", key),
        None => request,
    })
}

/// Sends `request` and `body` marked [`HOLDER_ONLY`] and reads the answer's head, even when
/// the body could not all be written: a runner without the link answers from the head alone
/// and closes before reading the body.
async fn try_holder(upstream: &mut TcpStream, request: &[u8], body: &[u8]) -> Tried {
    let strict = with_header(request, HOLDER_ONLY, "1");
    let sent = send(upstream, &strict, body).await;
    let Some((head, rest)) = read_head(upstream).await else {
        return Tried::Lost;
    };
    match status(&head) {
        Some(MISDIRECTED) => Tried::Elsewhere,
        _ if sent => Tried::Answered([head, rest].concat()),
        _ => Tried::Lost,
    }
}

/// The served host's upstream, or for the machine service the runner holding the Box's
/// machine's link when the ledger names one it can dial; a holder that cannot be dialled sends
/// the Proxy back to the ledger once. Why the Box gets a 502 when none answers.
async fn reach(service: &ServedRoute, route: &Route) -> Result<Reached, &'static str> {
    let ledger = &route.served.ledger;
    if service.served != Served::Machine {
        let own = vec![("X-Moor-Box", ledger.box_id().get().to_string())];
        return Ok(Reached {
            stream: dial_upstream(&service.upstream).await?,
            addr: service.upstream.clone(),
            own: Some(own),
            holder: false,
        });
    }
    let hold = tokio::task::block_in_place(|| hold_or_missing(ledger));
    let Some(addr) = holder(service, &hold) else {
        return Ok(Reached {
            stream: dial_upstream(&service.upstream).await?,
            addr: service.upstream.clone(),
            own: hold_headers(&hold),
            holder: false,
        });
    };
    match dial(&addr).await {
        Some(stream) => Ok(Reached {
            stream,
            addr,
            own: hold_headers(&hold),
            holder: true,
        }),
        None => reask(service, route, &addr).await,
    }
}

/// Asks the ledger again, without the holder at `failed` that just failed, and dials the holder
/// it names now; the service when it names none or the same one.
async fn reask(
    service: &ServedRoute,
    route: &Route,
    failed: &str,
) -> Result<Reached, &'static str> {
    let ledger = &route.served.ledger;
    let hold = tokio::task::block_in_place(|| {
        ledger.forget_holder();
        hold_or_missing(ledger)
    });
    let own = hold_headers(&hold);
    let addr = holder(service, &hold).filter(|addr| addr != failed);
    let Some(addr) = addr else {
        return Ok(Reached {
            stream: dial_upstream(&service.upstream).await?,
            addr: service.upstream.clone(),
            own,
            holder: false,
        });
    };
    Ok(Reached {
        stream: dial(&addr).await.ok_or(NOT_ANSWERING)?,
        addr,
        own,
        holder: false,
    })
}

/// A connection to a served host's `upstream`, looked up now and dialled only when every
/// address it names is loopback or private.
async fn dial_upstream(upstream: &str) -> Result<TcpStream, &'static str> {
    let addrs: Vec<SocketAddr> = timeout(MACHINE_DIAL, lookup_host(upstream))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(DOWN)?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| reachable(a.ip())) {
        return Err(PUBLIC);
    }
    timeout(MACHINE_DIAL, TcpStream::connect(addrs.as_slice()))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(DOWN)
}

/// The MCP address of the runner `hold` names, on the service's port: every runner serves MCP
/// on the port the service forwards to, and advertises only its links address. None for a
/// runner reached over a socket on this host, which the service already names.
fn holder(service: &ServedRoute, hold: &Hold) -> Option<String> {
    let Hold::Held(slot) = hold else {
        return None;
    };
    let runner = slot.runner.as_ref()?.as_str();
    let (host, _) = runner.rsplit_once(':').filter(|_| authority_ok(runner))?;
    let (_, port) = service.upstream.rsplit_once(':')?;
    Some(format!("{host}:{port}"))
}

/// A connection to `addr`, accepted within [`MACHINE_DIAL`].
async fn dial(addr: &str) -> Option<TcpStream> {
    timeout(MACHINE_DIAL, TcpStream::connect(addr))
        .await
        .ok()?
        .ok()
}

/// Writes `request` and `body` to `upstream`; whether both went.
async fn send(upstream: &mut TcpStream, request: &[u8], body: &[u8]) -> bool {
    write_within(upstream, request, WRITE_STALL).await
        && write_within(upstream, body, WRITE_STALL).await
}

/// Writes `bytes` to `upstream`, giving up once it takes no byte for `stall`. The limit is on
/// each write, not the whole, so a runner reading slowly still gets all of them.
async fn write_within(upstream: &mut TcpStream, mut bytes: &[u8], stall: Duration) -> bool {
    while !bytes.is_empty() {
        let Ok(Ok(n @ 1..)) = timeout(stall, upstream.write(bytes)).await else {
            return false;
        };
        bytes = bytes.get(n..).unwrap_or_default();
    }
    true
}

/// `request` with the header `name: value` added last.
fn with_header(request: &[u8], name: &str, value: &str) -> Vec<u8> {
    let mut request = request
        .get(..request.len().saturating_sub(2))
        .unwrap_or_default()
        .to_vec();
    request.extend_from_slice(format!("{name}: {value}\r\n\r\n").as_bytes());
    request
}

/// Reads from `ws` until `body` holds the whole body `request` frames by its length; false when
/// it has none that fits or the tunnel ends first, so the request cannot be sent twice.
async fn whole_body<S: Peer>(
    ws: &mut WebSocketStream<S>,
    request: &[u8],
    body: &mut Vec<u8>,
) -> bool {
    let Some(len) = body_len(request) else {
        return false;
    };
    let read = async {
        while body.len() < len {
            match ws.next().await? {
                Ok(Message::Binary(bytes)) => body.extend_from_slice(&bytes),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                _ => return None,
            }
        }
        Some(())
    };
    timeout(HEAD_TIMEOUT, read).await.ok().flatten().is_some() && body.len() == len
}

/// The body length `request` states, 0 without one; none when it is chunked or too large.
fn body_len(request: &[u8]) -> Option<usize> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Request::new(&mut headers);
    parsed.parse(request).ok()?;
    let mut len = 0;
    for h in parsed.headers.iter() {
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            return None;
        }
        if h.name.eq_ignore_ascii_case("content-length") {
            len = std::str::from_utf8(h.value).ok()?.trim().parse().ok()?;
        }
    }
    (len <= MAX_MESSAGE).then_some(len)
}

/// The status of the answer `head`.
fn status(head: &[u8]) -> Option<u16> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut headers);
    parsed.parse(head).ok()?;
    parsed.code
}

/// Answers 502 with `body` in the tunnel, once the Box's request was read.
async fn gateway_error<S: Peer>(ws: &mut WebSocketStream<S>, body: &str) {
    let reply = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    drop(ws.send(Message::binary(reply.into_bytes())).await);
    drop(ws.close(None).await);
}
