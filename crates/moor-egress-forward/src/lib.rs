//! The in-Box egress forwarder. tinyproxy sends it each CONNECT for a credential host, and it
//! carries that CONNECT to the shared egress Proxy as one WebSocket, the token in
//! `Authorization` and the target in `X-Moor-Target`. Each CONNECT dials afresh, so a Proxy
//! replica that drains costs only the tunnels it holds.
//!
//! The Proxy answers before it upgrades, so its status is the CONNECT's: `101` becomes
//! `200 Connection established`, and any other status goes to the client as it came.
//!
//! With no token in its config it serves a turn of a front in another pod: each client presents
//! its lease as the proxy password, and that goes to the Proxy as `Authorization: Basic`, since an
//! ingress may drop `Proxy-Authorization` as a hop-by-hop header. A client with none gets `407`.
//!
//! An absolute-form `http://` request goes the same way with `X-Moor-Plain: 1`. After `101` the
//! whole request goes into the tunnel, and the Proxy's reply comes back as sent.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use rustls::RootCertStore;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

mod target;
mod upstream;
use target::plain_target;
pub use upstream::Upstream;

/// The header naming the CONNECT target as `host:port`.
const TARGET: &str = "x-moor-target";
/// The header that marks a tunnel carrying one plain `http://` request.
const PLAIN: &str = "x-moor-plain";
/// The longest CONNECT head taken.
const HEAD_MAX: usize = 8 * 1024;
/// Most bytes read from the client per WebSocket message, well under the Proxy's 1 MiB cap.
const CHUNK: usize = 64 * 1024;
/// How long a CONNECT keeps dialing a Proxy it cannot reach, as while a replica stops listening
/// or a dev controller restarts and opens the Box's tunnel again.
const PATIENCE: Duration = Duration::from_secs(20);
/// The first pause between those dials; it doubles up to [`RETRY_MAX`].
const RETRY: Duration = Duration::from_millis(200);
/// The longest pause between those dials.
const RETRY_MAX: Duration = Duration::from_secs(1);
/// How long one dial, the upgrade included, may take.
const DIAL: Duration = Duration::from_secs(10);

/// The config file the controller writes into the Box.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    /// The loopback address tinyproxy sends credential hosts to.
    listen: SocketAddr,
    /// The Proxy's `/egress` address, `ws://` or `wss://`.
    upstream: String,
    /// The Box's token; none passes on each client's proxy password.
    #[serde(default)]
    token: Option<String>,
}

/// How the forwarder runs, read from its config file.
pub struct Config {
    /// Where it listens.
    pub listen: SocketAddr,
    /// Where it sends each CONNECT.
    pub upstream: Upstream,
}

impl Config {
    /// Parses the JSON config file, with the Proxy verified against the bundled web roots.
    ///
    /// # Errors
    /// The JSON is malformed, the upstream is not a `ws://` or `wss://` address, or the token
    /// is empty or cannot be sent in a header.
    pub fn parse(json: &str) -> Result<Self, String> {
        let file: File = serde_json::from_str(json).map_err(|e| format!("bad config: {e}"))?;
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let upstream = match file.token {
            Some(token) => Upstream::new(file.upstream, &token, roots)?,
            None => Upstream::passing_leases(file.upstream, roots)?,
        };
        Ok(Self {
            listen: file.listen,
            upstream,
        })
    }
}

/// The first pause after a failed accept.
const ACCEPT_PAUSE: Duration = Duration::from_millis(10);
/// The longest pause after failed accepts in a row.
const ACCEPT_PAUSE_MAX: Duration = Duration::from_secs(1);

/// The pause after `pause` when the next accept fails too.
#[must_use]
pub fn next_accept_pause(pause: Duration) -> Duration {
    (pause * 2).min(ACCEPT_PAUSE_MAX)
}

/// Serves each CONNECT accepted on `listener`. A failed accept, such as running out of file
/// descriptors, pauses from 10 ms, doubling up to 1 s, so the loop never spins.
pub async fn serve(listener: TcpListener, upstream: Arc<Upstream>) {
    let mut pause = ACCEPT_PAUSE;
    loop {
        if let Ok((client, _)) = listener.accept().await {
            pause = ACCEPT_PAUSE;
            let upstream = Arc::clone(&upstream);
            tokio::spawn(async move { forward(client, &upstream).await });
        } else {
            tokio::time::sleep(pause).await;
            pause = next_accept_pause(pause);
        }
    }
}

/// A CONNECT or plain request read from a client.
struct Connect {
    /// `host:port`.
    target: String,
    /// Bytes the client sent after a CONNECT head, such as a TLS hello, or the whole plain request.
    early: Vec<u8>,
    /// Whether it is a plain `http://` request.
    plain: bool,
    /// The client's `Basic` proxy credential.
    lease: Option<HeaderValue>,
}

/// A WebSocket open to the Proxy.
type Tunnel = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Why no tunnel opened.
enum Refused {
    /// The Proxy answered with this status and body.
    Status(StatusCode, Vec<u8>),
    /// The Proxy could not be reached.
    Unreachable(String),
}

/// Reads one CONNECT from `client`, opens its tunnel and copies bytes both ways.
async fn forward(mut client: TcpStream, upstream: &Upstream) {
    let connect = match read_connect(&mut client).await {
        Ok(Some(connect)) => connect,
        Ok(None) => return,
        Err(status) => return answer(&mut client, status, b"").await,
    };
    if upstream.bearer.is_none() && connect.lease.is_none() {
        return answer(&mut client, StatusCode::PROXY_AUTHENTICATION_REQUIRED, b"").await;
    }
    match dial(upstream, &connect).await {
        Ok(tunnel) => pump(client, tunnel, connect).await,
        Err(Refused::Status(status, body)) => answer(&mut client, status, &body).await,
        Err(Refused::Unreachable(why)) => {
            say(format_args!("CONNECT {}: {why}", connect.target));
            answer(&mut client, StatusCode::BAD_GATEWAY, why.as_bytes()).await;
        }
    }
}

/// The CONNECT head on `client`; `None` when it closes first.
async fn read_connect(client: &mut TcpStream) -> Result<Option<Connect>, StatusCode> {
    let mut buf = Vec::with_capacity(1024);
    let end = loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        if buf.len() > HEAD_MAX {
            return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
        }
        match client.read_buf(&mut buf).await {
            Ok(0) | Err(_) => return Ok(None),
            Ok(_) => {}
        }
    };
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(&buf), Ok(httparse::Status::Complete(_))) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = req.path.unwrap_or_default();
    let lease = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("proxy-authorization"))
        .filter(|h| h.value.starts_with(b"Basic "))
        .and_then(|h| HeaderValue::from_bytes(h.value).ok())
        .map(|mut value| {
            value.set_sensitive(true);
            value
        });
    if req.method != Some("CONNECT") {
        let target = plain_target(path)?;
        return Ok(Some(Connect {
            target,
            early: buf,
            plain: true,
            lease,
        }));
    }
    let target = Some(path)
        .filter(|t| {
            t.rsplit_once(':')
                .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok())
        })
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_owned();
    Ok(Some(Connect {
        target,
        early: buf.split_off(end),
        plain: false,
        lease,
    }))
}

/// Opens a WebSocket to the Proxy for `connect`, dialing again while it cannot be reached.
async fn dial(upstream: &Upstream, connect: &Connect) -> Result<Tunnel, Refused> {
    let until = tokio::time::Instant::now() + upstream.patience;
    let mut pause = RETRY;
    loop {
        let last = match tokio::time::timeout(DIAL, dial_once(upstream, connect)).await {
            Ok(Ok(tunnel)) => return Ok(tunnel),
            Ok(Err(Refused::Unreachable(why))) => why,
            Ok(Err(refused)) => return Err(refused),
            Err(_) => format!("no answer from {} in {}s", upstream.url, DIAL.as_secs()),
        };
        if tokio::time::Instant::now() + pause > until {
            return Err(Refused::Unreachable(last));
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(RETRY_MAX);
    }
}

/// One dial of [`dial`].
async fn dial_once(upstream: &Upstream, connect: &Connect) -> Result<Tunnel, Refused> {
    let mut req = upstream
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| Refused::Unreachable(format!("bad upstream {}: {e}", upstream.url)))?;
    let headers = req.headers_mut();
    if let Some(auth) = upstream.bearer.as_ref().or(connect.lease.as_ref()) {
        headers.insert("authorization", auth.clone());
    }
    headers.insert(
        TARGET,
        HeaderValue::from_str(&connect.target)
            .map_err(|_| Refused::Status(StatusCode::BAD_REQUEST, Vec::new()))?,
    );
    if connect.plain {
        headers.insert(PLAIN, HeaderValue::from_static("1"));
    }
    let tls = Connector::Rustls(Arc::clone(&upstream.tls));
    match tokio_tungstenite::connect_async_tls_with_config(req, None, true, Some(tls)).await {
        Ok((tunnel, _)) => Ok(tunnel),
        Err(WsError::Http(res)) => {
            let status = res.status();
            Err(Refused::Status(status, res.into_body().unwrap_or_default()))
        }
        Err(e) => Err(Refused::Unreachable(format!(
            "cannot reach {}: {e}",
            upstream.url
        ))),
    }
}

/// Tells a CONNECT client its tunnel is open, then copies bytes between it and `tunnel`, its
/// early bytes first, until the tunnel ends.
async fn pump(mut client: TcpStream, tunnel: Tunnel, connect: Connect) {
    if !connect.plain
        && client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .is_err()
    {
        return;
    }
    let early = connect.early;
    let (mut sink, mut stream) = tunnel.split();
    let (mut from_client, mut to_client) = client.into_split();
    let up = async move {
        if !early.is_empty() && sink.send(Message::binary(early)).await.is_err() {
            return;
        }
        let mut buf = vec![0_u8; CHUNK];
        while let Ok(n) = from_client.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let chunk = buf.get(..n).unwrap_or_default().to_vec();
            if sink.send(Message::binary(chunk)).await.is_err() {
                break;
            }
        }
    };
    let down = async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Binary(bytes) => {
                    if to_client.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Ping(_) | Message::Pong(_) => {}
                _ => break,
            }
        }
        drop(to_client.shutdown().await);
    };
    tokio::pin!(down);
    tokio::select! {
        () = &mut down => {}
        () = up => down.await,
    }
}

/// Answers `client` with `status` and `body`, then closes.
async fn answer(client: &mut TcpStream, status: StatusCode, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        body.len()
    );
    drop(client.write_all(head.as_bytes()).await);
    drop(client.write_all(body).await);
    drop(client.shutdown().await);
}

/// Writes one `moor-egress-forward: ` line to stderr.
pub fn say(args: fmt::Arguments<'_>) {
    use std::io::Write as _;
    drop(writeln!(std::io::stderr(), "moor-egress-forward: {args}"));
}
