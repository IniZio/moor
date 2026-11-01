//! `moor mcp`: a stdio MCP server that relays each message to a moor server's `/mcp`, signed in
//! with the shared sign-in, so any agent tool that runs local MCP servers reaches the gate.
//!
//! It relays messages instead of serving MCP itself: the gate is stateless, so there is no
//! session to mirror, and the client sees the gate's answers and protocol version unchanged.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use clap::Parser;
use moor_client::session::{normalize, Store, Tokens};
use moor_client::sse::Messages;
use moor_client::TokenSource;
use serde_json::{json, Value};
use ureq::http::Response;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body};

/// `moor mcp`'s flags.
#[derive(Debug, Parser)]
#[command(name = "moor mcp")]
struct Args {
    /// The server, as its origin or its /mcp address.
    #[arg(long, env = "MOOR_SERVER")]
    server: String,
}

/// The `_meta` key a 2026-07-28 message names its protocol version under, as a JSON pointer.
const META_VERSION: &str = "/params/_meta/io.modelcontextprotocol~1protocolVersion";

/// Runs `moor mcp` until stdin ends; `None` when `args` names another command.
#[must_use]
pub fn run(args: &[OsString]) -> Option<ExitCode> {
    if args.get(1)?.to_str()? != "mcp" {
        return None;
    }
    let parsed = match Args::try_parse_from(args.iter().skip(1)) {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = e.print();
            return Some(ExitCode::from(if e.use_stderr() { 2 } else { 0 }));
        }
    };
    let relay = Arc::new(Relay::new(&parsed.server));
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let relay = Arc::clone(&relay);
        std::thread::spawn(move || relay.relay(&line));
    }
    Some(ExitCode::SUCCESS)
}

/// The gate one `moor mcp` relays to.
struct Relay {
    /// HTTP client; carries TLS.
    agent: Agent,
    /// The server's origin, which keys its sign-in.
    origin: String,
    /// The `/mcp` endpoint.
    url: String,
    /// The shared sign-in's access tokens.
    tokens: Tokens,
    /// The version `initialize` settled, sent as `MCP-Protocol-Version` from then on.
    version: Mutex<Option<String>>,
}

impl Relay {
    /// A relay to `server`; nothing is sent yet.
    fn new(server: &str) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .build();
        let origin = normalize(server);
        Self {
            agent: Agent::new_with_config(config),
            url: format!("{origin}/mcp"),
            tokens: Tokens::new(Store::user(), &origin),
            origin,
            version: Mutex::new(None),
        }
    }

    /// Relays one line from the client; a failed request answers with a JSON-RPC error.
    fn relay(&self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            Self::write(&error(&Value::Null, -32700, "not a JSON-RPC message"));
            return;
        };
        if let Err(why) = self.exchange(&message) {
            match message
                .get("id")
                .filter(|_| message.get("method").is_some())
            {
                Some(id) => Self::write(&error(id, -32000, &why)),
                None => log(&why),
            }
        }
    }

    /// POSTs `message`, refreshing the token once if the gate refuses it, and writes the answer.
    fn exchange(&self, message: &Value) -> Result<(), String> {
        let token = self.tokens.bearer()?;
        let answer = match self.post(message, &token)? {
            Some(answer) => answer,
            None => self
                .post(message, &self.tokens.renewed(&token)?)?
                .ok_or_else(|| {
                    format!(
                        "{} refused the sign-in; run `moor login --server {}`",
                        self.origin, self.origin
                    )
                })?,
        };
        self.answer(message, answer)
    }

    /// The gate's response to `message` sent with `token`; `None` when it refused the token.
    fn post(&self, message: &Value, token: &str) -> Result<Option<Response<Body>>, String> {
        let mut req = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        let version = message
            .pointer(META_VERSION)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| lock(&self.version).clone());
        if let Some(version) = version {
            req = req.header("MCP-Protocol-Version", version);
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            req = req.header("Mcp-Method", method);
        }
        let params = message.get("params");
        let name = params.and_then(|p| p.get("name").or_else(|| p.get("uri")));
        if let Some(name) = name.and_then(Value::as_str) {
            req = req.header("Mcp-Name", name);
        }
        let res = req
            .send(message.to_string())
            .map_err(|e| format!("cannot reach {}: {e}", self.url))?;
        Ok((res.status().as_u16() != 401).then_some(res))
    }

    /// Writes each message of the gate's answer to `sent` as it arrives.
    fn answer(&self, sent: &Value, res: Response<Body>) -> Result<(), String> {
        let status = res.status();
        let events = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let mut body = res.into_body();
        if events {
            for message in Messages::new(BufReader::new(body.into_reader())) {
                let message = message.map_err(|e| format!("{}: {e}", self.url))?;
                self.note(sent, &message);
                Self::write(&message);
            }
            return Ok(());
        }
        let text = body
            .read_to_string()
            .map_err(|e| format!("{}: {e}", self.url))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(message) if message.get("jsonrpc").is_some() => {
                self.note(sent, &message);
                Self::write(&message);
                Ok(())
            }
            _ if status.is_success() => Ok(()),
            _ => Err(format!("{} answered {status}: {}", self.url, text.trim())),
        }
    }

    /// Keeps the version the gate settled when `answer` answers `initialize`.
    fn note(&self, sent: &Value, answer: &Value) {
        if sent.get("method").and_then(Value::as_str) != Some("initialize") {
            return;
        }
        if let Some(version) = answer
            .pointer("/result/protocolVersion")
            .and_then(Value::as_str)
        {
            *lock(&self.version) = Some(version.to_owned());
        }
    }

    /// Writes `message` to stdout as one line, holding stdout so lines never interleave.
    fn write(message: &Value) {
        let wrote = {
            let mut out = io::stdout().lock();
            writeln!(out, "{message}").and_then(|()| out.flush())
        };
        if let Err(e) = wrote {
            log(&format!("cannot write to stdout: {e}"));
        }
    }
}

/// A JSON-RPC error answering `id`.
fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// `mutex`'s value, also after a writer panicked.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Logs `line` to stderr, which an agent tool keeps apart from the MCP messages on stdout.
fn log(line: &str) {
    let _ = writeln!(io::stderr(), "moor mcp: {line}");
}
