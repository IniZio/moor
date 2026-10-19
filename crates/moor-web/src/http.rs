//! Loopback HTTP/1.1 server: request parsing, authentication, CSRF checks and a pluggable handler.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::auth::{self, Credentials, CsrfHeaders, Via};

mod limits;
pub use limits::Limits;

#[cfg(test)]
mod limit_tests;
#[cfg(test)]
mod tests;

/// Largest request head accepted, in bytes.
const MAX_HEAD: usize = 16 * 1024;

/// Largest request body accepted, in bytes.
const MAX_BODY: usize = 64 * 1024;

/// How long a client may take to send its request or accept a write.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Headers added to every response.
const COMMON_HEADERS: &str = "Connection: close\r\n\
X-Content-Type-Options: nosniff\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'\r\n\
Referrer-Policy: no-referrer\r\n";

/// A parsed request.
#[derive(Debug)]
pub struct HttpRequest {
    /// Request method, as sent.
    pub method: String,
    /// Path without the query string.
    pub path: String,
    /// Raw query string, empty when absent.
    query: String,
    /// Header names (lowercased) with their values.
    headers: Vec<(String, String)>,
    /// Body bytes; empty when the request sent no `content-length`.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Value of the header `name` (lowercase), if sent.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Value of the query parameter `key`; values are taken verbatim, not percent-decoded.
    #[must_use]
    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Value of the cookie `name`, if sent.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v)
    }
}

/// A response with a fixed body.
#[derive(Debug)]
pub struct HttpResponse {
    /// Status line text, such as `200 OK`.
    pub status: &'static str,
    /// `Content-Type` value.
    pub content_type: &'static str,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
    /// Body text.
    pub body: String,
}

impl HttpResponse {
    /// A plain-text response.
    #[must_use]
    pub fn text(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// A `200 OK` HTML response.
    #[must_use]
    pub fn html(body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            body: body.into(),
        }
    }
}

/// Answers authenticated requests; POSTs have also passed the CSRF checks.
pub type Handler = Arc<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>;

/// A running server.
#[derive(Debug)]
pub struct Server {
    /// Address the listener is bound to.
    addr: SocketAddr,
}

impl Server {
    /// Address the server listens on.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
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
            HttpResponse::html("<!doctype html><title>moor</title><h1>moor</h1>")
        } else {
            HttpResponse::text("404 Not Found", "not found")
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
    thread::Builder::new()
        .name("moor-web".into())
        .spawn(move || {
            for mut stream in listener.incoming().flatten() {
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
    Ok(Server { addr })
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
    let reply = answer(&req, token, handler);
    let extra: Vec<(&str, &str)> = reply
        .headers
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    respond(
        &mut stream,
        reply.status,
        reply.content_type,
        &extra,
        &reply.body,
    )
}

/// Authenticates and screens `req`, then runs the handler.
fn answer(req: &HttpRequest, token: &str, handler: &Handler) -> HttpResponse {
    let creds = Credentials {
        method: &req.method,
        path: &req.path,
        bearer: req
            .header("authorization")
            .and_then(|v| v.strip_prefix("Bearer ")),
        cookie: req.cookie(auth::COOKIE),
        query_token: req.query_param("token"),
    };
    let unauthorized = || HttpResponse::text("401 Unauthorized", "unauthorized");
    let Some(via) = auth::authenticate(&creds, token) else {
        return unauthorized();
    };
    match req.method.as_str() {
        "GET" => {
            let mut reply = handler(req);
            if matches!(via, Via::Query) {
                reply
                    .headers
                    .push(("Set-Cookie", auth::cookie_header(token)));
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
                Err(why) => HttpResponse::text("403 Forbidden", format!("forbidden: {why}")),
            }
        }
        _ => {
            let mut reply = HttpResponse::text("405 Method Not Allowed", "method not allowed");
            reply.headers.push(("Allow", "GET, POST".to_owned()));
            reply
        }
    }
}

/// Reads and parses one request, head and body.
///
/// # Errors
/// Fails on I/O errors or timeout, an oversized head, early EOF or a malformed head.
/// Body failures are told apart by kind: `InvalidInput` means the body is over
/// `MAX_BODY` (answer 413); `InvalidData` (bad `content-length`, chunked
/// encoding) and `UnexpectedEof` (connection closed mid-body) mean 400.
fn read_request(stream: &mut impl Read) -> io::Result<HttpRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(invalid("request head too large"));
        }
        let n = stream.read(&mut chunk)?;
        let got = chunk.get(..n).unwrap_or_default();
        if got.is_empty() {
            return Err(invalid("connection closed before end of head"));
        }
        buf.extend_from_slice(got);
    };
    let mut body = buf.split_off(head_end);
    let mut req = parse_head(&String::from_utf8_lossy(&buf))?;
    if req.header("transfer-encoding").is_some() {
        return Err(invalid("transfer-encoding is not supported"));
    }
    let Some(len) = req.header("content-length") else {
        return Ok(req);
    };
    let len: usize = len.parse().map_err(|_| invalid("bad content-length"))?;
    if len > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request body too large",
        ));
    }
    // Bytes past the declared length are not ours to keep.
    body.truncate(len);
    while body.len() < len {
        let want = (len - body.len()).min(chunk.len());
        let n = stream.read(chunk.get_mut(..want).unwrap_or_default())?;
        let got = chunk.get(..n).unwrap_or_default();
        if got.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before end of body",
            ));
        }
        body.extend_from_slice(got);
    }
    req.body = body;
    Ok(req)
}

/// Parses the text of a request head.
fn parse_head(text: &str) -> io::Result<HttpRequest> {
    let mut lines = text.split("\r\n");
    let mut parts = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid("malformed request line"));
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Ok(HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body: Vec::new(),
    })
}

/// An `InvalidData` error.
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
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
    body: &str,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{COMMON_HEADERS}",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head.push_str(body);
    out.write_all(head.as_bytes())?;
    out.flush()
}
