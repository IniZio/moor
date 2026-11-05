//! The in-Box egress forwarder. tinyproxy sends it each CONNECT for a credential host, and it
//! carries that CONNECT to the shared egress Proxy as one WebSocket, the token in
//! `Authorization` and the target in `X-Moor-Target`. Each CONNECT dials afresh, so a Proxy
//! replica that drains costs only the tunnels it holds.
//!
//! The Proxy answers before it upgrades, so its status is the CONNECT's: `101` becomes
//! `200 Connection established`, and any other status goes to the client as it came.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

/// The header naming the CONNECT target as `host:port`.
const TARGET: &str = "x-moor-target";
/// The longest CONNECT head taken.
const HEAD_MAX: usize = 8 * 1024;
/// Most bytes read from the client per WebSocket message, well under the Proxy's 1 MiB cap.
const CHUNK: usize = 64 * 1024;
/// Dials per CONNECT when the Proxy cannot be reached, as while a replica stops listening.
const ATTEMPTS: u32 = 3;
/// Pause between those dials.
const RETRY: Duration = Duration::from_millis(200);
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
    /// The Box's token.
    token: String,
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
        Ok(Self {
            listen: file.listen,
            upstream: Upstream::new(file.upstream, &file.token, roots)?,
        })
    }
}

/// The shared Proxy's `/egress` address and the token presented there.
pub struct Upstream {
    /// `ws://` or `wss://` address.
    url: String,
    /// `Bearer <token>`.
    bearer: HeaderValue,
    /// TLS for a `wss://` address.
    tls: Arc<ClientConfig>,
}

impl fmt::Debug for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upstream")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Upstream {
    /// `url` with `token`, trusting `roots` for `wss://`.
    ///
    /// # Errors
    /// As for [`Config::parse`].
    pub fn new(url: String, token: &str, roots: RootCertStore) -> Result<Self, String> {
        match (url.strip_prefix("ws://"), url.starts_with("wss://")) {
            (Some(rest), _) if !loopback(rest) => {
                return Err(format!(
                    "upstream {url} sends the token in the clear; only a loopback ws:// address may"
                ))
            }
            (Some(_), _) | (None, true) => {}
            (None, false) => {
                return Err(format!("upstream {url} is not a ws:// or wss:// address"))
            }
        }
        if token.is_empty() {
            return Err("the token is empty".to_owned());
        }
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "the token cannot be sent in a header".to_owned())?;
        bearer.set_sensitive(true);
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self {
            url,
            bearer,
            tls: Arc::new(tls),
        })
    }
}

/// Whether the host of `rest`, an address after its scheme, is a loopback name or address.
fn loopback(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or_default();
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or_default(),
        |v6| v6.split(']').next().unwrap_or_default(),
    );
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
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

/// A CONNECT read from a client.
struct Connect {
    /// `host:port`.
    target: String,
    /// Bytes the client sent after the head, such as a TLS hello.
    early: Vec<u8>,
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
    match dial(upstream, &connect.target).await {
        Ok(tunnel) => pump(client, tunnel, connect.early).await,
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
    if req.method != Some("CONNECT") {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let target = req
        .path
        .filter(|t| {
            t.rsplit_once(':')
                .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok())
        })
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_owned();
    Ok(Some(Connect {
        target,
        early: buf.split_off(end),
    }))
}

/// Opens a WebSocket to the Proxy for `target`, dialing again while it cannot be reached.
async fn dial(upstream: &Upstream, target: &str) -> Result<Tunnel, Refused> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(RETRY).await;
        }
        match tokio::time::timeout(DIAL, dial_once(upstream, target)).await {
            Ok(Ok(tunnel)) => return Ok(tunnel),
            Ok(Err(Refused::Unreachable(why))) => last = why,
            Ok(Err(refused)) => return Err(refused),
            Err(_) => last = format!("no answer from {} in {}s", upstream.url, DIAL.as_secs()),
        }
    }
    Err(Refused::Unreachable(last))
}

/// One dial of [`dial`].
async fn dial_once(upstream: &Upstream, target: &str) -> Result<Tunnel, Refused> {
    let mut req = upstream
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| Refused::Unreachable(format!("bad upstream {}: {e}", upstream.url)))?;
    let headers = req.headers_mut();
    headers.insert("authorization", upstream.bearer.clone());
    headers.insert(
        TARGET,
        HeaderValue::from_str(target)
            .map_err(|_| Refused::Status(StatusCode::BAD_REQUEST, Vec::new()))?,
    );
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

/// Tells `client` its tunnel is open, then copies bytes between it and `tunnel`, `early` first,
/// until the tunnel ends.
async fn pump(mut client: TcpStream, tunnel: Tunnel, early: Vec<u8>) {
    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
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
