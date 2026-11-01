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
    /// The controller's front socket.
    front_socket: PathBuf,
}

impl Mcp {
    /// An MCP surface at `resource_url` whose tools act on `front_socket`, on a gate without
    /// browser sign-in.
    ///
    /// # Errors
    /// When the URL has no host or the async runtime cannot start.
    pub fn new(resource_url: &str, front_socket: PathBuf) -> Result<Self, String> {
        Self::with_setup(resource_url, front_socket, None)
    }

    /// An MCP surface like [`Mcp::new`] whose webhook replies link to the setup page at
    /// `public_url` when the gate serves browser sign-in there.
    ///
    /// # Errors
    /// When the URL has no host or the async runtime cannot start.
    pub fn with_setup(
        resource_url: &str,
        front_socket: PathBuf,
        public_url: Option<&str>,
    ) -> Result<Self, String> {
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
        let tools = Tools {
            front_socket: front_socket.clone(),
            public_url: public_url.map(|url| url.trim_end_matches('/').into()),
        };
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
            front_socket,
        })
    }

    /// The RFC 9728 document for this resource.
    pub(crate) fn metadata(&self, issuer: &str) -> String {
        serde_json::json!({
            "resource": self.resource_url,
            "authorization_servers": [issuer],
            // Clients request what is listed here, and an OIDC issuer refuses a request without openid.
            "scopes_supported": ["openid", "offline_access", READ, WRITE],
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
        let authorization = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let verified = tokio::task::block_in_place(|| self.authenticate(authorization, gate));
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

    /// Who an `Authorization` header speaks for and the scopes it grants, or why it is refused.
    pub(crate) fn authenticate(
        &self,
        authorization: Option<&str>,
        gate: &Gate,
    ) -> Result<(Person, Vec<String>), String> {
        let token = authorization
            .and_then(bearer)
            .ok_or_else(|| "send an access token as Authorization: Bearer <token>".to_owned())?;
        gate.verifier.verify_resource(token, &self.resource_url)
    }

    /// The `WWW-Authenticate` challenge that points a client at the metadata document.
    pub(crate) fn challenge(&self) -> String {
        format!("Bearer resource_metadata=\"{}\"", self.metadata_url())
    }

    /// The controller's front socket.
    pub(crate) fn front_socket(&self) -> &std::path::Path {
        &self.front_socket
    }

    /// The 401 that points a client at the metadata document.
    fn unauthorized(&self, why: &str) -> Response<Body> {
        let challenge = self.challenge();
        let mut res = Response::new(Full::new(Bytes::from(format!("{why}\n"))).boxed());
        *res.status_mut() = StatusCode::UNAUTHORIZED;
        if let Ok(value) = challenge.parse() {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
        }
        res
    }
}

/// The bearer token in the value of an `Authorization` header.
fn bearer(value: &str) -> Option<&str> {
    value.strip_prefix("Bearer ").map(str::trim)
}

/// Who a request speaks for and what it may do; set by the gate, never by the client.
#[derive(Clone)]
pub(crate) struct Auth {
    /// The person.
    pub(crate) person: Person,
    /// The scopes the token grants.
    pub(crate) scopes: Vec<String>,
}
