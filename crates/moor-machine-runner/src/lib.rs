//! An MCP server on the host through which a Box uses one of its user's machines over ssh.
//!
//! The Box's Proxy routes `machine.moor.internal` here and names the caller in three headers:
//! the Box id, the machine's name and its ssh target. `sync` mirrors the Box's work tree onto
//! the machine; `exec` runs a command there, in that copy.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use moor_client::Client;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use tokio::net::TcpListener;

use crate::caller::Caller;
use crate::exec::{ExecArgs, ExecOutput};
use crate::sync::SyncOutput;

mod caller;
mod exec;
mod sync;

/// What the runner needs to serve.
#[derive(Debug, Clone)]
pub struct Config {
    /// The controller's socket, which `sync` asks for the Box's work tree.
    pub socket: PathBuf,
    /// Where the runner keeps one host mirror per Box.
    pub state_dir: PathBuf,
    /// The `Host` the Box's Proxy passes through, accepted besides the loopback names.
    pub service_host: String,
}

/// Serves MCP over streamable HTTP on `listener` until accepting fails.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve(listener: TcpListener, config: Config) -> io::Result<()> {
    let runner = Runner {
        client: Client::new(&config.socket),
        mirrors: config.state_dir.join("mirrors"),
        syncing: Arc::default(),
    };
    let http = StreamableHttpServerConfig::default()
        .with_allowed_hosts([
            "localhost",
            "127.0.0.1",
            "::1",
            config.service_host.as_str(),
        ])
        .with_json_response(true);
    let service = StreamableHttpService::new(
        move || Ok(runner.clone()),
        Arc::new(LocalSessionManager::default()),
        http,
    );
    loop {
        let (stream, _) = listener.accept().await?;
        let service = TowerToHyperService::new(service.clone());
        tokio::spawn(async move {
            let _ = auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

/// The MCP server: one per request, sharing the per-Box sync locks.
#[derive(Clone)]
struct Runner {
    /// Client for the controller's operator socket.
    client: Client,
    /// Parent of the per-Box host mirrors.
    mirrors: PathBuf,
    /// One lock per Box id, so two syncs of a Box never write its mirror at once.
    syncing: Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>>,
}

#[tool_router]
impl Runner {
    /// Mirrors the calling Box's work tree onto its machine.
    #[tool(
        description = "Copy this Box's work tree to the attached machine at ~/moor/<box id>, so \
                       the machine sees your current files. .git is left out; node_modules is \
                       kept. Later syncs send only what changed, and remove files you deleted."
    )]
    async fn sync(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<Json<SyncOutput>, ErrorData> {
        let caller = Caller::from_headers(&parts.headers)?;
        let lock = {
            let mut locks = self
                .syncing
                .lock()
                .map_err(|_| internal("sync locks poisoned"))?;
            Arc::clone(locks.entry(caller.box_id.get()).or_default())
        };
        let _held = lock.lock().await;
        let (client, mirrors) = (self.client.clone(), self.mirrors.clone());
        tokio::task::spawn_blocking(move || sync::run(&client, &mirrors, &caller))
            .await
            .map_err(|e| internal(&e.to_string()))?
            .map(Json)
            .map_err(|e| internal(&e))
    }

    /// Runs a shell command on the machine, in the synced copy of the work tree.
    #[tool(
        description = "Run a shell command on the attached machine, in ~/moor/<box id> (call \
                       sync first), through the machine user's login shell. Returns the exit \
                       code and the last 64 KiB of combined stdout and stderr. timeout_secs \
                       defaults to 600, at most 3600; on timeout the command is killed."
    )]
    async fn exec(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(args): Parameters<ExecArgs>,
    ) -> Result<Json<ExecOutput>, ErrorData> {
        let caller = Caller::from_headers(&parts.headers)?;
        let secs = args.timeout_secs.unwrap_or(exec::DEFAULT_TIMEOUT_SECS);
        if secs == 0 || secs > exec::MAX_TIMEOUT_SECS {
            return Err(ErrorData::invalid_params(
                format!(
                    "timeout_secs must be 1 to {}, not {secs}",
                    exec::MAX_TIMEOUT_SECS
                ),
                None,
            ));
        }
        exec::run(&caller, &args.command, Duration::from_secs(secs))
            .await
            .map(Json)
            .map_err(|e| internal(&format!("could not run ssh: {e}")))
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the generated handlers are async whether or not they await"
)]
#[tool_handler]
impl ServerHandler for Runner {}

/// An MCP internal error carrying `message`.
fn internal(message: &str) -> ErrorData {
    ErrorData::internal_error(message.to_owned(), None)
}
