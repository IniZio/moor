//! One MCP gate: JSON-RPC calls and the listen stream, with a bearer token from a [`Bearer`].

use std::io::{BufReader, Cursor, Read};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use crate::idle::Idle;
use crate::sse::Messages;
use crate::token::{Bearer, Fail};

/// The MCP version whose `subscriptions/listen` the gate serves.
const PROTOCOL: &str = "2026-07-28";

/// Longest a listen stream lives. It bounds how long a given-up reader thread lingers, and the
/// reconnect it forces prints nothing.
const LIFE: Duration = Duration::from_secs(600);

/// Time limit of one call and of connecting.
const CALL: Duration = Duration::from_secs(30);

/// A messages stream of an open listen.
pub(crate) type Stream = Messages<BufReader<Box<dyn Read + Send>>>;

/// The gate at `url`, signed in by `bearer`.
pub(crate) struct Gate {
    /// HTTP client; carries TLS.
    agent: Agent,
    /// The `/mcp` endpoint.
    url: String,
    /// Where tokens come from.
    bearer: Box<dyn Bearer>,
    /// Silence after which a listen stream counts as dead.
    idle: Duration,
}

impl Gate {
    /// A gate client; nothing is sent yet.
    pub(crate) fn new(url: String, bearer: Box<dyn Bearer>, idle: Duration) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_connect(Some(CALL))
            .build();
        Self {
            agent: Agent::new_with_config(config),
            url,
            bearer,
            idle,
        }
    }

    /// The token to connect with.
    pub(crate) fn token(&self) -> Result<String, Fail> {
        self.bearer.bearer()
    }

    /// A new token, after the gate refused the last.
    pub(crate) fn renew(&self) -> Result<String, Fail> {
        self.bearer.renew()
    }

    /// POSTs `method`; the answer is the first message that has an `id`, or the stream.
    fn post(
        &self,
        token: &str,
        method: &str,
        mut params: Value,
        long: bool,
    ) -> Result<Stream, Fail> {
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL,
            "io.modelcontextprotocol/clientInfo": {
                "name": "moor-listen", "version": env!("CARGO_PKG_VERSION")},
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        if let Some(object) = params.as_object_mut() {
            object.insert("_meta".to_owned(), meta);
        }
        let mut req = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL)
            .header("Mcp-Method", method);
        if let Some(uri) = params.get("uri").and_then(Value::as_str) {
            req = req.header("Mcp-Name", uri);
        }
        let limit = if long { LIFE } else { CALL };
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let res = req
            .config()
            .timeout_recv_body(Some(limit))
            .timeout_recv_response(Some(CALL))
            .build()
            .send(body.to_string())
            .map_err(|e| Fail::other(&e))?;
        match res.status().as_u16() {
            401 | 403 => return Err(Fail::Rejected),
            200..=299 => {}
            other => return Err(Fail::other(&format!("HTTP {other}"))),
        }
        let events = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let mut body = res.into_body();
        let reader: Box<dyn Read + Send> = if events && long {
            Box::new(Idle::new(body.into_reader(), self.idle))
        } else if events {
            Box::new(body.into_reader())
        } else {
            // A plain JSON answer reads as one event, so callers parse one shape.
            let text = body.read_to_string().map_err(|e| Fail::other(&e))?;
            let framed = text.lines().collect::<Vec<_>>().join("\ndata: ");
            Box::new(Cursor::new(format!("data: {framed}\n\n")))
        };
        Ok(Messages::new(BufReader::new(reader)))
    }

    /// Calls `method` and returns its `result`.
    pub(crate) fn call(&self, token: &str, method: &str, params: Value) -> Result<Value, Fail> {
        let mut stream = self.post(token, method, params, false)?;
        for message in &mut stream {
            let message = message.map_err(|e| Fail::other(&e))?;
            if let Some(error) = message.get("error") {
                return Err(Fail::other(&format!("{method}: {error}")));
            }
            if let Some(result) = message.get("result") {
                return Ok(result.clone());
            }
        }
        Err(Fail::other(&format!("{method}: no answer")))
    }

    /// Opens a listen on the resource list and on `uris`.
    pub(crate) fn listen(&self, token: &str, uris: &[String]) -> Result<Stream, Fail> {
        let params = json!({"notifications": {
            "resourcesListChanged": true,
            "resourceSubscriptions": uris,
        }});
        self.post(token, "subscriptions/listen", params, true)
    }
}
