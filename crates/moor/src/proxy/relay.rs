//! Relays an admitted request to the route's upstream over HTTPS.
//!
//! The client body and the upstream body are both streamed through fixed
//! buffers. Redirects are returned to the client unchanged, never followed.

mod capped;
mod chunked;
#[cfg(test)]
mod tests;

use std::io::{BufReader, Cursor, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::proxy_gate::{Inject, Plan, RouteId};
use ureq::config::Config;
use ureq::http::{HeaderName, HeaderValue, Method, Request, Response};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body, SendBody};

use self::capped::{is_cap_exceeded, CappedReader};
use self::chunked::ChunkedReader;
use super::head::Head;
use super::server::{Forward, ForwardHandler};
use super::upstream::{auth_value, RouteCredential, Upstreams};

/// Size of the buffer the response body is copied through.
const COPY_BUF: usize = 8 * 1024;
/// How long connecting and resolving may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the upstream may take to answer with a response head.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(600);
/// Response headers that never cross the proxy.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
/// Request headers the relay sets itself from the body framing.
const FRAMING: [&str; 2] = ["content-length", "expect"];

/// Why a request could not be relayed.
enum Failure {
    /// The route has no usable credential.
    Credential,
    /// The upstream could not be reached or the request could not be built.
    Unreachable,
    /// The client's body framing is unusable.
    Framing,
    /// The client's body is larger than the route allows.
    TooLarge,
}

/// Largest request body the route accepts, in bytes. The listener applies
/// the same caps to a declared `content-length`.
fn body_cap(route: &RouteId) -> u64 {
    if route.as_str() == "forge.git" {
        2 * 1024 * 1024 * 1024
    } else {
        32 * 1024 * 1024
    }
}

/// Sends admitted requests to their upstream and streams the answer back.
pub(crate) struct Relay {
    /// Base URL per route.
    upstreams: Upstreams,
    /// Source of route credentials.
    creds: Arc<dyn RouteCredential>,
    /// Upstream client; follows no redirects and uses no proxy.
    agent: Agent,
}

impl Relay {
    /// Builds a relay over `upstreams` that injects `creds`.
    pub(crate) fn new(upstreams: Upstreams, creds: Arc<dyn RouteCredential>) -> Self {
        Self {
            upstreams,
            creds,
            agent: build_agent(),
        }
    }

    /// Sends the request upstream and returns the response head and body.
    fn call(
        &self,
        head: &Head,
        plan: &Plan,
        prefix: Vec<u8>,
        stream: &mut TcpStream,
    ) -> Result<Response<Body>, Failure> {
        let url = self
            .upstreams
            .url(&plan.route, &plan.upstream_path)
            .ok_or(Failure::Unreachable)?;
        let auth = self.authorization(&plan.route, plan.inject)?;
        let method =
            Method::from_bytes(plan.method.as_bytes()).map_err(|_| Failure::Unreachable)?;
        let framing = Framing::of(head).ok_or(Failure::Framing)?;
        if matches!(framing, Framing::Length(n) if n > body_cap(&plan.route)) {
            return Err(Failure::TooLarge);
        }
        let mut builder = Request::builder().method(method).uri(url);
        for (name, value) in &plan.headers {
            if !FRAMING.iter().any(|f| name.eq_ignore_ascii_case(f)) {
                builder = builder.header(name, value);
            }
        }
        if let Framing::Length(n) = framing {
            builder = builder.header("content-length", n);
        }
        if let Some(value) = auth {
            builder = builder.header("authorization", value);
        }
        let request = builder.body(()).map_err(|_| Failure::Unreachable)?;
        let (parts, ()) = request.into_parts();
        let cap = body_cap(&plan.route);
        let tripped = Arc::new(AtomicBool::new(false));
        let result = match framing {
            Framing::None => self.agent.run(Request::from_parts(parts, SendBody::none())),
            Framing::Length(n) => {
                let mut body = Cursor::new(prefix).chain(stream).take(n);
                self.agent
                    .run(Request::from_parts(parts, SendBody::from_reader(&mut body)))
            }
            Framing::Chunked => {
                let decoded = ChunkedReader::new(BufReader::new(Cursor::new(prefix).chain(stream)));
                let mut body = CappedReader::new(decoded, cap, tripped.clone());
                self.agent
                    .run(Request::from_parts(parts, SendBody::from_reader(&mut body)))
            }
        };
        result.map_err(|err| {
            if tripped.load(Ordering::SeqCst)
                || matches!(&err, ureq::Error::Io(io) if is_cap_exceeded(io))
            {
                Failure::TooLarge
            } else {
                Failure::Unreachable
            }
        })
    }

    /// Resolves the credential into a sensitive `Authorization` value.
    fn authorization(
        &self,
        route: &RouteId,
        inject: Inject,
    ) -> Result<Option<HeaderValue>, Failure> {
        if inject == Inject::None {
            return Ok(None);
        }
        let secret = self
            .creds
            .credential(route)
            .map_err(|_| Failure::Credential)?;
        let full = auth_value(inject, &secret).ok_or(Failure::Credential)?;
        let mut value = HeaderValue::from_bytes(full.expose()).map_err(|_| Failure::Credential)?;
        value.set_sensitive(true);
        Ok(Some(value))
    }
}

impl ForwardHandler for Relay {
    fn forward(&self, req: Forward, stream: &mut TcpStream) -> u16 {
        let Forward {
            head,
            plan,
            body_prefix,
        } = req;
        let route = plan.route.clone();
        let status = match self.call(&head, &plan, body_prefix, stream) {
            Ok(response) => write_response(stream, response),
            Err(failure) => write_failure(stream, &failure, &route),
        };
        let _ = stream.shutdown(Shutdown::Write);
        status
    }
}

/// How the client body is delimited.
#[derive(Clone, Copy)]
enum Framing {
    /// No body.
    None,
    /// Exactly this many bytes.
    Length(u64),
    /// Chunked transfer coding.
    Chunked,
}

impl Framing {
    /// Reads the framing from the client's request headers; `None` when a
    /// `content-length` is not a plain number.
    fn of(head: &Head) -> Option<Self> {
        let mut length = None;
        for (name, value) in &head.headers {
            if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                return Some(Self::Chunked);
            }
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<u64>().ok()?);
            }
        }
        Some(length.map_or(Self::None, Self::Length))
    }
}

/// Builds the upstream client.
fn build_agent() -> Agent {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let config = Config::builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .proxy(None)
        .user_agent(ureq::config::AutoHeaderValue::None)
        .accept(ureq::config::AutoHeaderValue::None)
        .accept_encoding(ureq::config::AutoHeaderValue::None)
        .tls_config(tls)
        .timeout_resolve(Some(CONNECT_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .build();
    Agent::new_with_config(config)
}

/// Writes the upstream response head and streams its body; returns the status.
fn write_response(stream: &mut TcpStream, response: Response<Body>) -> u16 {
    let (parts, mut body) = response.into_parts();
    let code = parts.status.as_u16();
    let named: Vec<String> = parts
        .headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        code,
        parts.status.canonical_reason().unwrap_or("Unknown")
    )
    .into_bytes();
    for (name, value) in &parts.headers {
        if !is_hop(name, &named) {
            head.extend_from_slice(name.as_str().as_bytes());
            head.extend_from_slice(b": ");
            head.extend_from_slice(value.as_bytes());
            head.extend_from_slice(b"\r\n");
        }
    }
    head.extend_from_slice(b"connection: close\r\n\r\n");
    if stream.write_all(&head).is_err() || stream.flush().is_err() {
        return code;
    }
    copy_body(&mut body.as_reader(), stream);
    code
}

/// Whether `name` is hop-by-hop or listed in the `connection` header.
fn is_hop(name: &HeaderName, named: &[String]) -> bool {
    HOP_BY_HOP.contains(&name.as_str()) || named.iter().any(|t| t == name.as_str())
}

/// Copies `from` to `to` through a fixed buffer, flushing after every read.
fn copy_body(from: &mut impl Read, to: &mut TcpStream) {
    let mut buf = [0_u8; COPY_BUF];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let Some(chunk) = buf.get(..n) else { return };
                if to.write_all(chunk).is_err() || to.flush().is_err() {
                    return;
                }
            }
        }
    }
}

/// Writes a fixed-text error response; returns its status.
fn write_failure(stream: &mut TcpStream, failure: &Failure, route: &RouteId) -> u16 {
    let (code, reason, body) = match failure {
        Failure::Credential => (
            502,
            "Bad Gateway",
            format!(
                "upstream credential unavailable for route {}\n",
                route.as_str()
            ),
        ),
        Failure::Unreachable => (
            502,
            "Bad Gateway",
            format!("upstream unreachable for route {}\n", route.as_str()),
        ),
        Failure::Framing => (
            400,
            "Bad Request",
            "unusable request body framing\n".to_owned(),
        ),
        Failure::TooLarge => (
            413,
            "Content Too Large",
            "request body exceeds the route limit\n".to_owned(),
        ),
    };
    let text = format!(
        "HTTP/1.1 {code} {reason}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
    code
}
