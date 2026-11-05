//! One connection: the upgrade request, the CONNECT into the Box's proxy, then the tunnel.
//!
//! Request bytes carry the Box token, so nothing here logs them.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use zeroize::Zeroizing;

use super::boxes::{Refused, Route};
use super::{Shared, PATH, TARGET};
use crate::proxy::front::connect_head;
use crate::store::token_hash;

/// Largest request or answer head read.
const MAX_HEAD: usize = 8 * 1024;
/// How long a peer may take to send its head, and the Box's proxy to answer the CONNECT.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest WebSocket message or frame taken from the forwarder.
const MAX_MESSAGE: usize = 1 << 20;
/// Bytes read from the target per message.
const CHUNK: usize = 16 * 1024;

/// What the forwarder asked for.
struct Ask {
    /// The request path.
    path: String,
    /// `Sec-WebSocket-Key`, when it asked to upgrade.
    key: Option<String>,
    /// The bearer token.
    token: Option<Zeroizing<Vec<u8>>>,
    /// `X-Moor-Target`, when it is `host:port`.
    target: Option<String>,
}

/// What a request head asks the connection to do.
enum Checked {
    /// Answer the readiness probe.
    Ready,
    /// Open a tunnel to `target` through `route`, upgrading with `key`.
    Tunnel {
        /// `Sec-WebSocket-Key`.
        key: String,
        /// `host:port`.
        target: String,
        /// The Box's proxy.
        route: Route,
    },
}

/// Serves one connection to its end.
pub(super) async fn serve(mut stream: TcpStream, shared: &Shared) {
    let Some((head, _)) = read_head(&mut stream).await else {
        return;
    };
    match check(&head, shared) {
        Err((status, why)) => answer(&mut stream, status, &why).await,
        Ok(Checked::Ready) => answer(&mut stream, 200, "ok").await,
        Ok(Checked::Tunnel { key, target, route }) => tunnel(stream, &key, &target, &route).await,
    }
}

/// Opens the CONNECT, upgrades the connection and relays the tunnel.
async fn tunnel(mut stream: TcpStream, key: &str, target: &str, route: &Route) {
    let (upstream, rest) = match open(route, target).await {
        Ok(opened) => opened,
        Err(status) => return answer(&mut stream, status, "the Box's proxy refused").await,
    };
    let accept = derive_accept_key(key.as_bytes());
    let switching = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(switching.as_bytes()).await.is_err() {
        return;
    }
    let limits = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let ws = WebSocketStream::from_raw_socket(stream, Role::Server, Some(limits)).await;
    relay(ws, upstream, rest).await;
}

/// What `head` asks for, or the status and reason it is refused with.
fn check(head: &[u8], shared: &Shared) -> Result<Checked, (u16, String)> {
    let refuse = |status: u16, why: &str| (status, why.to_owned());
    let ask = parse(head).ok_or_else(|| refuse(400, "bad request"))?;
    match ask.path.as_str() {
        "/ready" => return Ok(Checked::Ready),
        PATH => {}
        _ => return Err(refuse(404, "not found")),
    }
    let (Some(key), Some(target)) = (ask.key, ask.target) else {
        return Err(refuse(400, "expected a WebSocket upgrade with a target"));
    };
    let token = ask.token.ok_or_else(|| refuse(401, "no token"))?;
    if !shared.boxes.signed(&token) {
        return Err(refuse(401, "unknown token"));
    }
    let hash = token_hash(&token);
    let route = admit(shared, &hash).map_err(|refused| match refused {
        Refused::Unknown => refuse(401, "unknown token"),
        Refused::Limited => refuse(429, "too many tunnels; retry after a second"),
        Refused::Unavailable(why) => {
            shared
                .boxes
                .say(format_args!("a tunnel was refused: {why}"));
            refuse(503, "temporarily unavailable")
        }
    })?;
    Ok(Checked::Tunnel { key, target, route })
}

/// Looks the token up off the async workers, since the ledger and a proxy start block.
fn admit(shared: &Shared, hash: &str) -> Result<Route, Refused> {
    tokio::task::block_in_place(|| shared.boxes.admit(hash))
}

/// Opens the CONNECT to `target` through the Box's proxy; `Err` carries the status to answer.
async fn open(route: &Route, target: &str) -> Result<(TcpStream, Vec<u8>), u16> {
    let served = &route.served;
    let mut upstream = TcpStream::connect(served.addr).await.map_err(|_| 503_u16)?;
    let head = connect_head(target, served.internal.expose());
    upstream.write_all(&head).await.map_err(|_| 503_u16)?;
    let (answer, rest) = read_head(&mut upstream).await.ok_or(502_u16)?;
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Response::new(&mut headers);
    let code = match parsed.parse(&answer) {
        Ok(httparse::Status::Complete(_)) => parsed.code.unwrap_or(502),
        _ => 502,
    };
    if (200..300).contains(&code) {
        Ok((upstream, rest))
    } else {
        Err(code)
    }
}

/// Copies tunnel bytes both ways until each side ends; `rest` came after the CONNECT answer.
async fn relay(ws: WebSocketStream<TcpStream>, upstream: TcpStream, rest: Vec<u8>) {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (mut up_rx, mut up_tx) = upstream.into_split();
    let down = async {
        if !rest.is_empty() && ws_tx.send(Message::binary(rest)).await.is_err() {
            return;
        }
        let mut buf = vec![0_u8; CHUNK];
        loop {
            match up_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = buf.get(..n).unwrap_or_default().to_vec();
                    if ws_tx.send(Message::binary(chunk)).await.is_err() {
                        return;
                    }
                }
            }
        }
        drop(ws_tx.close().await);
    };
    let up = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(bytes) => {
                    if up_tx.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                Message::Text(_) | Message::Close(_) => break,
            }
        }
        drop(up_tx.shutdown().await);
    };
    tokio::join!(down, up);
}

/// Answers `status` with `body` and closes.
async fn answer(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Refused",
    };
    let retry = if status == 429 {
        "Retry-After: 1\r\n"
    } else {
        ""
    };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         {retry}Connection: close\r\n\r\n{body}",
        body.len()
    );
    drop(stream.write_all(reply.as_bytes()).await);
    drop(stream.shutdown().await);
}

/// Reads up to the blank line ending a head; the head and the bytes read past it.
async fn read_head(stream: &mut TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
    let read = async {
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 2048];
        loop {
            let n = stream.read(&mut chunk).await.ok().filter(|n| *n > 0)?;
            buf.extend_from_slice(chunk.get(..n)?);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let rest = buf.split_off(pos + 4);
                return Some((buf, rest));
            }
            if buf.len() > MAX_HEAD {
                return None;
            }
        }
    };
    timeout(HEAD_TIMEOUT, read).await.ok().flatten()
}

/// The forwarder's request, or `None` when it is not one HTTP/1.1 request head.
fn parse(head: &[u8]) -> Option<Ask> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(head), Ok(httparse::Status::Complete(_))) || req.method != Some("GET") {
        return None;
    }
    let find = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value)
    };
    let text = |name: &str| find(name).and_then(|v| std::str::from_utf8(v).ok());
    let upgrade = text("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let token = find("authorization")
        .and_then(|v| v.strip_prefix(b"Bearer "))
        .filter(|t| !t.is_empty())
        .map(|t| Zeroizing::new(t.to_vec()));
    Some(Ask {
        path: req.path?.to_owned(),
        key: text("sec-websocket-key")
            .filter(|_| upgrade)
            .map(str::to_owned),
        token,
        target: text(TARGET).filter(|t| authority_ok(t)).map(str::to_owned),
    })
}

/// True for `host:port` or `[v6]:port` with a port that fits and no path, userinfo or spacing.
pub(super) fn authority_ok(authority: &str) -> bool {
    let plain = authority
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b'@' | b'?' | b'#' | b'\\'));
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let host_ok = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .map_or_else(
            || !host.is_empty() && !host.contains([':', '[', ']']),
            |v6| v6.parse::<std::net::Ipv6Addr>().is_ok(),
        );
    plain && host_ok && port.parse::<u16>().is_ok()
}
