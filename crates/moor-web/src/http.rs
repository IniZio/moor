//! Loopback HTTP/1.1 server: request parsing, authentication, CSRF checks and a pluggable handler.

use std::borrow::Cow;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::auth::{self, Credentials, CsrfHeaders, Via};

mod download;
pub use download::{Download, Fill};
mod limits;
pub use limits::Limits;

#[cfg(test)]
mod limit_tests;
mod parse;
#[cfg(test)]
mod shutdown_tests;
#[cfg(test)]
mod tests;

use download::send_download;
use parse::read_request;
pub use parse::HttpRequest;
#[cfg(test)]
use parse::MAX_BODY;

/// How long a client may take to send its request or accept a write.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Headers added to every response.
const COMMON_HEADERS: &str = "Connection: close\r\n\
X-Content-Type-Options: nosniff\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'\r\n\
Referrer-Policy: no-referrer\r\n";

/// A response with a fixed body.
#[derive(Debug)]
pub struct HttpResponse {
    /// Status line text, such as `200 OK`.
    pub status: &'static str,
    /// `Content-Type` value.
    pub content_type: Cow<'static, str>,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
    /// Body bytes, sent unchanged.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A plain-text response.
    #[must_use]
    pub fn text(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: Cow::Borrowed("text/plain"),
            headers: Vec::new(),
            body: body.into().into_bytes(),
        }
    }

    /// A `200 OK` HTML response.
    #[must_use]
    pub fn html(body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            content_type: Cow::Borrowed("text/html; charset=utf-8"),
            headers: Vec::new(),
            body: body.into().into_bytes(),
        }
    }

    /// A response carrying raw bytes under a content type known only at run time.
    #[must_use]
    pub const fn binary(
        status: &'static str,
        content_type: String,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            content_type: Cow::Owned(content_type),
            headers,
            body,
        }
    }
}

/// Writes a streaming body after the head is sent; returns when the stream ends or the peer leaves.
pub type StreamBody = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

/// What a handler answers with.
pub enum Reply {
    /// A complete response.
    Full(HttpResponse),
    /// An event stream; the closure writes the body.
    Stream(StreamBody),
    /// A body streamed from elsewhere, such as an artifact read from the controller.
    Download(Download),
}

impl Reply {
    /// The extra headers of a reply whose head the handler controls.
    const fn headers_mut(&mut self) -> Option<&mut Vec<(&'static str, String)>> {
        match self {
            Self::Full(resp) => Some(&mut resp.headers),
            Self::Download(d) => Some(&mut d.headers),
            Self::Stream(_) => None,
        }
    }
}

impl From<HttpResponse> for Reply {
    fn from(resp: HttpResponse) -> Self {
        Self::Full(resp)
    }
}

/// Answers authenticated requests; POSTs have also passed the CSRF checks.
pub type Handler = Arc<dyn Fn(&HttpRequest) -> Reply + Send + Sync>;

/// A running server.
#[derive(Debug)]
pub struct Server {
    /// Address the listener is bound to.
    addr: SocketAddr,
    /// Set to make the accept loop stop.
    stop: Arc<AtomicBool>,
    /// The accept loop.
    accept: JoinHandle<()>,
    /// Counts the connections being served.
    gate: limits::Gate,
}

impl Server {
    /// Address the server listens on.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting connections, then waits up to `grace` for the ones being served to
    /// finish. A request still running after `grace`, such as an open event stream, is left
    /// to die with the process.
    pub fn shutdown(self, grace: Duration) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        let _ = self.accept.join();
        self.gate.wait_idle(grace);
    }

    /// The URL of the page root.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.addr.port())
    }
}

/// Handler used until the front serves real pages: a placeholder root, 404 elsewhere.
#[must_use]
pub fn placeholder_handler() -> Handler {
    Arc::new(|req| {
        if req.method == "GET" && req.path == "/" {
            HttpResponse::html("<!doctype html><title>moor</title><h1>moor</h1>").into()
        } else {
            HttpResponse::text("404 Not Found", "not found").into()
        }
    })
}

/// Binds `127.0.0.1:port` and serves in background threads.
///
/// # Errors
/// Fails when the port cannot be bound.
pub fn start(port: u16, token: String, handler: Handler) -> io::Result<Server> {
    start_with(port, token, handler, Limits::default())
}

/// Like [`start`], with explicit connection limits.
///
/// # Errors
/// Fails when the port cannot be bound.
pub fn start_with(
    port: u16,
    token: String,
    handler: Handler,
    limits: Limits,
) -> io::Result<Server> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let addr = listener.local_addr()?;
    let token = Arc::new(token);
    let gate = limits::Gate::default();
    let stop = Arc::new(AtomicBool::new(false));
    let (accept_stop, accept_gate) = (Arc::clone(&stop), gate.clone());
    let accept = thread::Builder::new()
        .name("moor-web".into())
        .spawn(move || {
            let gate = accept_gate;
            for mut stream in listener.incoming().flatten() {
                if accept_stop.load(Ordering::SeqCst) {
                    break;
                }
                let Some(slot) = gate.admit(&mut stream, limits.max_connections) else {
                    continue;
                };
                let (token, handler) = (Arc::clone(&token), Arc::clone(&handler));
                let _ = thread::Builder::new()
                    .name("moor-web-conn".into())
                    .spawn(move || {
                        let _slot = slot;
                        let _ = serve(stream, &token, &handler, limits);
                    });
            }
        })?;
    Ok(Server {
        addr,
        stop,
        accept,
        gate,
    })
}

/// Handles one connection.
fn serve(mut stream: TcpStream, token: &str, handler: &Handler, lim: Limits) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let req = match read_request(&mut limits::Deadline::new(&stream, lim.read_deadline)) {
        Ok(req) => req,
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            return respond(
                &mut stream,
                "413 Payload Too Large",
                "text/plain",
                &[],
                "payload too large",
            );
        }
        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
            return limits::timed_out(&mut stream);
        }
        Err(_) => {
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                &[],
                "bad request",
            )
        }
    };
    let reply = match answer(&req, token, handler) {
        Reply::Full(reply) => reply,
        Reply::Stream(body) => {
            begin_stream(&mut stream)?;
            return body(&mut stream);
        }
        Reply::Download(download) => match send_download(&mut stream, download)? {
            Some(failed) => failed,
            None => return Ok(()),
        },
    };
    let extra: Vec<(&str, &str)> = reply
        .headers
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    respond(
        &mut stream,
        reply.status,
        &reply.content_type,
        &extra,
        &reply.body,
    )
}

/// Authenticates and screens `req`, then runs the handler.
fn answer(req: &HttpRequest, token: &str, handler: &Handler) -> Reply {
    let creds = Credentials {
        method: &req.method,
        path: &req.path,
        bearer: req
            .header("authorization")
            .and_then(|v| v.strip_prefix("Bearer ")),
        cookie: req.cookie(auth::COOKIE),
        query_token: req.query_param("token"),
    };
    let unauthorized = || HttpResponse::text("401 Unauthorized", "unauthorized").into();
    let Some(via) = auth::authenticate(&creds, token) else {
        return unauthorized();
    };
    match req.method.as_str() {
        "GET" => {
            let mut reply = handler(req);
            if let (Some(headers), Via::Query) = (reply.headers_mut(), via) {
                headers.push(("Set-Cookie", auth::cookie_header(token)));
            }
            reply
        }
        "POST" => {
            if matches!(via, Via::Query) {
                return unauthorized();
            }
            let headers = CsrfHeaders {
                csrf: req.header("x-moor-csrf"),
                content_type: req.header("content-type"),
                host: req.header("host"),
                origin: req.header("origin"),
                sec_fetch_site: req.header("sec-fetch-site"),
            };
            match auth::check_csrf(&headers) {
                Ok(()) => handler(req),
                Err(why) => HttpResponse::text("403 Forbidden", format!("forbidden: {why}")).into(),
            }
        }
        _ => {
            let mut reply = HttpResponse::text("405 Method Not Allowed", "method not allowed");
            reply.headers.push(("Allow", "GET, POST".to_owned()));
            reply.into()
        }
    }
}

/// Writes a complete response with a fixed body.
///
/// # Errors
/// Fails when the write fails.
fn respond(
    out: &mut impl Write,
    status: &str,
    content_type: &str,
    extra: &[(&str, &str)],
    body: impl AsRef<[u8]>,
) -> io::Result<()> {
    let body = body.as_ref();
    write_head(out, status, content_type, body.len() as u64, extra)?;
    out.write_all(body)?;
    out.flush()
}

/// Writes a response head announcing a body of `len` bytes.
///
/// # Errors
/// Fails when the write fails.
fn write_head(
    out: &mut impl Write,
    status: &str,
    content_type: &str,
    len: u64,
    extra: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {len}\r\n{COMMON_HEADERS}",
        no_breaks(content_type),
    );
    for (k, v) in extra {
        head.push_str(&no_breaks(k));
        head.push_str(": ");
        head.push_str(&no_breaks(v));
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())
}

/// Drops CR and LF so a header field cannot start a new header or end the head.
fn no_breaks(field: &str) -> Cow<'_, str> {
    if field.contains(['\r', '\n']) {
        Cow::Owned(field.replace(['\r', '\n'], ""))
    } else {
        Cow::Borrowed(field)
    }
}

/// Writes the head of an event stream, with no length so the body can run on.
///
/// # Errors
/// Fails when the write fails.
fn begin_stream(out: &mut impl Write) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n{COMMON_HEADERS}\r\n"
    );
    out.write_all(head.as_bytes())?;
    out.flush()
}
