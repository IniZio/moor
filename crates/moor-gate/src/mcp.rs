//! `/mcp`: a streamable HTTP MCP server on the gate. A bot signs in as a person through the
//! issuer; every request carries that person's access token, and each tool acts on the
//! controller's front socket as that person, through the same rewrite `/front` uses.

use std::convert::Infallible;
use std::io::Cursor;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use http::{header, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::{service_fn, Service};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::io::{join, split, AsyncReadExt};
use tokio::runtime::Runtime;

use crate::tools::Tools;
use crate::{log, Gate, Person};

/// Scope that lets a token read Boxes.
pub(crate) const READ: &str = "moor:read";

/// Scope that lets a token start, steer and close Boxes.
pub(crate) const WRITE: &str = "moor:write";

/// Where a client learns which issuer to sign in with.
pub(crate) const METADATA_PATH: &str = "/.well-known/oauth-protected-resource";

/// The response body type the MCP service answers with.
type Body = BoxBody<Bytes, Infallible>;

/// The MCP surface of one gate.
pub struct Mcp {
    /// The public URL of `/mcp`; a token's `aud` must be exactly this.
    resource_url: String,
    /// Runs the connections that reach `/mcp`.
    runtime: Runtime,
    /// The MCP service, shared by every connection so sessions survive reconnects.
    service: StreamableHttpService<Tools, LocalSessionManager>,
}

impl Mcp {
    /// An MCP surface at `resource_url` whose tools act on `front_socket`.
    ///
    /// # Errors
    /// When the URL has no host or the async runtime cannot start.
    pub fn new(resource_url: &str, front_socket: PathBuf) -> Result<Self, String> {
        let host = resource_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split('/').next())
            .filter(|host| !host.is_empty())
            .ok_or_else(|| format!("{resource_url} is not an absolute URL"))?;
        let hosts = ["localhost", "127.0.0.1", "::1", host];
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(hosts)
            .with_legacy_session_mode(false)
            .with_json_response(true);
        let tools = Tools { front_socket };
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the MCP runtime: {e}"))?;
        Ok(Self {
            resource_url: resource_url.to_owned(),
            runtime,
            service,
        })
    }

    /// The RFC 9728 document for this resource.
    pub(crate) fn metadata(&self, issuer: &str) -> String {
        serde_json::json!({
            "resource": self.resource_url,
            "authorization_servers": [issuer],
            "scopes_supported": [READ, WRITE],
            "bearer_methods_supported": ["header"],
        })
        .to_string()
    }

    /// Where the metadata document is: next to the resource's host.
    fn metadata_url(&self) -> String {
        let origin = self
            .resource_url
            .split_once("://")
            .map(|(scheme, rest)| {
                let host = rest.split('/').next().unwrap_or_default();
                format!("{scheme}://{host}")
            })
            .unwrap_or_default();
        format!("{origin}{METADATA_PATH}")
    }

    /// Serves `stream`, whose request head (`prefix`) the gate already read, until it closes.
    pub(crate) fn serve(&self, stream: TcpStream, prefix: Vec<u8>, gate: &Gate) {
        if stream.set_read_timeout(None).is_err() || stream.set_nonblocking(true).is_err() {
            return;
        }
        self.runtime.block_on(async {
            let Ok(stream) = tokio::net::TcpStream::from_std(stream) else {
                return;
            };
            // The gate already consumed the request head; replay it ahead of the socket.
            let (reader, writer) = split(stream);
            let io = TokioIo::new(join(Cursor::new(prefix).chain(reader), writer));
            let svc = service_fn(|req| async { Ok::<_, Infallible>(self.route(req, gate).await) });
            let _ = http1::Builder::new().serve_connection(io, svc).await;
        });
    }

    /// Checks the request's token, then lets the MCP service answer it.
    async fn route(&self, mut req: Request<Incoming>, gate: &Gate) -> Response<Body> {
        let verified = bearer(&req)
            .ok_or_else(|| "send an access token as Authorization: Bearer <token>".to_owned())
            .and_then(|token| {
                tokio::task::block_in_place(|| {
                    gate.verifier.verify_resource(token, &self.resource_url)
                })
            });
        match verified {
            Ok((person, scopes)) => {
                req.extensions_mut().insert(Auth { person, scopes });
                match TowerToHyperService::new(self.service.clone())
                    .call(req)
                    .await
                {
                    Ok(res) => res,
                    Err(never) => match never {},
                }
            }
            Err(why) => {
                if !why.starts_with("send an access token") {
                    log(&format!("/mcp refused a token: {why}"));
                }
                self.unauthorized(&why)
            }
        }
    }

    /// The 401 that points a client at the metadata document.
    fn unauthorized(&self, why: &str) -> Response<Body> {
        let challenge = format!("Bearer resource_metadata=\"{}\"", self.metadata_url());
        let mut res = Response::new(Full::new(Bytes::from(format!("{why}\n"))).boxed());
        *res.status_mut() = StatusCode::UNAUTHORIZED;
        if let Ok(value) = challenge.parse() {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
        }
        res
    }
}

/// The bearer token of `req`.
fn bearer(req: &Request<Incoming>) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
}

/// Who a request speaks for and what it may do; set by the gate, never by the client.
#[derive(Clone)]
pub(crate) struct Auth {
    /// The person.
    pub(crate) person: Person,
    /// The scopes the token grants.
    pub(crate) scopes: Vec<String>,
}
