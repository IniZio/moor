//! Optional loopback web UI: a live Task list behind a token.

mod actions;
mod http;
#[cfg(test)]
mod tests;

use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::view::TaskView;
use sha2::{Digest, Sha256};

use http::{begin_stream, read_request, respond, Request as Http};

/// Source of the data the page shows.
pub(crate) trait Feed {
    /// Current Task list.
    ///
    /// # Errors
    /// Returns a message when the store cannot be read.
    fn tasks(&self) -> Result<Vec<TaskView>, String>;
    /// Up to `limit` events with a cursor greater than `after`, oldest first.
    ///
    /// # Errors
    /// Returns a message when the store cannot be read.
    fn events_after(&self, after: i64, limit: usize) -> Result<Vec<ServerFrame>, String>;
    /// Names of the controller's known repos (`repos:` config keys), sorted.
    fn repos(&self) -> Vec<String>;
    /// Runs `req` exactly as a socket `request` frame would; the CLI's envelope.
    fn call(&self, req: Request) -> Response;
}

/// Builds a [`Feed`] on the connection thread; the real feed is not `Send`.
pub(crate) type FeedFactory = Arc<dyn Fn() -> Result<Box<dyn Feed>, String> + Send + Sync>;

/// A running web server.
pub(crate) struct Web {
    /// Address the listener is bound to.
    addr: SocketAddr,
}

impl Web {
    /// Address the server listens on.
    pub(crate) const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// How long a client may take to send its request head or accept a write.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle time after which an event stream sends a comment, so dead clients surface as write errors.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Events fetched per store read.
const BATCH: usize = 256;

/// Per-server state shared with connection threads.
struct Ctx {
    /// The token every request must present.
    token: String,
    /// Builds the per-connection feed.
    feed: FeedFactory,
    /// Pause between store reads once an event stream has caught up.
    poll: Duration,
}

/// Binds `127.0.0.1:port` and serves in background threads.
///
/// # Errors
/// Fails when the port cannot be bound.
pub(crate) fn start(
    port: u16,
    token: String,
    feed: FeedFactory,
    poll: Duration,
) -> io::Result<Web> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let addr = listener.local_addr()?;
    let ctx = Arc::new(Ctx { token, feed, poll });
    thread::Builder::new()
        .name("moor-web".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let ctx = Arc::clone(&ctx);
                let _ = thread::Builder::new()
                    .name("moor-web-conn".into())
                    .spawn(move || {
                        let _ = serve(stream, &ctx);
                    });
            }
        })?;
    Ok(Web { addr })
}

/// Handles one connection.
fn serve(mut stream: TcpStream, ctx: &Ctx) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let req = match read_request(&mut stream) {
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
        Err(_) => {
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                &[],
                "bad request",
            );
        }
    };
    let Some(via_query) = authenticate(&req, &ctx.token) else {
        return respond(
            &mut stream,
            "401 Unauthorized",
            "text/plain",
            &[],
            "unauthorized",
        );
    };
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/" | "/app.js" | "/tasks" | "/events" | "/repos") => {}
        ("POST", p) if actions::is_action(p) => return post(&mut stream, &req, ctx),
        (_, "/" | "/app.js" | "/tasks" | "/events" | "/repos") => {
            return not_allowed(&mut stream, "GET");
        }
        (_, p) if actions::is_action(p) => return not_allowed(&mut stream, "POST"),
        _ => return respond(&mut stream, "404 Not Found", "text/plain", &[], "not found"),
    }
    route(&mut stream, &req, ctx, via_query)
}

/// Answers a wrong-method request on a known path.
fn not_allowed(out: &mut TcpStream, allow: &str) -> io::Result<()> {
    respond(
        out,
        "405 Method Not Allowed",
        "text/plain",
        &[("Allow", allow)],
        "method not allowed",
    )
}

/// Answers an authenticated POST.
fn post(out: &mut TcpStream, req: &Http, ctx: &Ctx) -> io::Result<()> {
    let reply = match (ctx.feed)() {
        Ok(feed) => actions::handle(req, feed.as_ref()),
        Err(e) => actions::Reply::text("500 Internal Server Error", e),
    };
    respond(out, reply.status, reply.content_type, &[], &reply.body)
}

/// Checks the request's credentials; `Some(true)` when it authenticated by query token.
fn authenticate(req: &Http, token: &str) -> Option<bool> {
    let bearer = req
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "));
    if bearer.is_some_and(|t| token_matches(token, t))
        || req
            .cookie("moor_token")
            .is_some_and(|t| token_matches(token, t))
    {
        return Some(false);
    }
    let on_page = req.method == "GET" && req.path == "/";
    let query = req.query_param("token").filter(|_| on_page);
    query
        .is_some_and(|t| token_matches(token, t))
        .then_some(true)
}

/// Compares two tokens without an early exit on the first differing byte.
fn token_matches(expected: &str, given: &str) -> bool {
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.as_bytes());
    let diff = a
        .iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    !expected.is_empty() && diff == 0
}

/// Dispatches an authenticated GET.
fn route(out: &mut TcpStream, req: &Http, ctx: &Ctx, via_query: bool) -> io::Result<()> {
    match req.path.as_str() {
        "/" => {
            let cookie = format!(
                "moor_token={}; HttpOnly; SameSite=Strict; Path=/",
                ctx.token
            );
            let extra: &[(&str, &str)] = if via_query {
                &[("Set-Cookie", &cookie)]
            } else {
                &[]
            };
            respond(
                out,
                "200 OK",
                "text/html; charset=utf-8",
                extra,
                include_str!("web/page.html"),
            )
        }
        "/app.js" => respond(
            out,
            "200 OK",
            "application/javascript; charset=utf-8",
            &[],
            include_str!("web/app.js"),
        ),
        "/tasks" => tasks(out, ctx),
        "/repos" => repos(out, ctx),
        "/events" => events(out, req, ctx),
        _ => respond(out, "404 Not Found", "text/plain", &[], "not found"),
    }
}

/// Answers `/tasks`.
fn tasks(out: &mut TcpStream, ctx: &Ctx) -> io::Result<()> {
    let listed = (ctx.feed)().and_then(|f| f.tasks());
    match listed.and_then(|t| {
        serde_json::to_string(&serde_json::json!({ "tasks": t })).map_err(|e| e.to_string())
    }) {
        Ok(body) => respond(out, "200 OK", "application/json", &[], &body),
        Err(e) => respond(out, "500 Internal Server Error", "text/plain", &[], &e),
    }
}

/// Answers `/repos`.
fn repos(out: &mut TcpStream, ctx: &Ctx) -> io::Result<()> {
    match (ctx.feed)() {
        Ok(f) => {
            let body = serde_json::json!({ "repos": f.repos() }).to_string();
            respond(out, "200 OK", "application/json", &[], &body)
        }
        Err(e) => respond(out, "500 Internal Server Error", "text/plain", &[], &e),
    }
}

/// Answers `/events` with a server-sent event stream.
fn events(out: &mut TcpStream, req: &Http, ctx: &Ctx) -> io::Result<()> {
    let start = req
        .header("last-event-id")
        .or_else(|| req.query_param("since"))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let feed = match (ctx.feed)() {
        Ok(f) => f,
        Err(e) => return respond(out, "500 Internal Server Error", "text/plain", &[], &e),
    };
    begin_stream(out)?;
    stream_events(out, feed.as_ref(), start, ctx.poll)
}

/// Pushes events after `cursor` until the client goes away or the feed fails.
fn stream_events(
    out: &mut impl Write,
    feed: &dyn Feed,
    mut cursor: i64,
    poll: Duration,
) -> io::Result<()> {
    let mut idle = Duration::ZERO;
    loop {
        let frames = match feed.events_after(cursor, BATCH) {
            Ok(f) => f,
            Err(e) => {
                let msg = e.replace(['\r', '\n'], " ");
                return out.write_all(format!("event: error\ndata: {msg}\n\n").as_bytes());
            }
        };
        let before = cursor;
        for frame in &frames {
            cursor = send_event(out, frame, cursor)?;
        }
        if cursor != before {
            idle = Duration::ZERO;
            continue;
        }
        idle += poll;
        if idle >= KEEPALIVE {
            idle = Duration::ZERO;
            out.write_all(b": keepalive\n\n")?;
            out.flush()?;
        }
        thread::sleep(poll);
    }
}

/// Writes one event frame; returns the cursor after it.
fn send_event(out: &mut impl Write, frame: &ServerFrame, cursor: i64) -> io::Result<i64> {
    let ServerFrame::Event { cursor: id, .. } = frame else {
        return Ok(cursor);
    };
    let data = serde_json::to_string(frame).map_err(io::Error::other)?;
    out.write_all(format!("id: {id}\ndata: {data}\n\n").as_bytes())?;
    out.flush()?;
    Ok(*id)
}
