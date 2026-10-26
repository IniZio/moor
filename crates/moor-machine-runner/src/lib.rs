//! An MCP server on the host through which a Box uses one of its user's machines over ssh.
//!
//! The Box's Proxy routes `machine.moor.internal` here and names the caller in four headers:
//! the Box id, the machine's name, its ssh target and the Box's slot on it. `sync` mirrors the
//! Box's work tree onto the machine; `exec` runs a command there, in that copy, holding any
//! capped resources it names; `machine_info` says which slot and ports the Box has.

use std::collections::{BTreeMap, HashMap};
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
use rmcp::model::ProgressNotificationParam;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use serde::Serialize;
use tokio::net::TcpListener;

use crate::caller::Caller;
use crate::exec::{ExecArgs, ExecOutput};
use crate::holds::Holds;
use crate::sync::SyncOutput;

mod caller;
mod exec;
mod holds;
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
    /// Units of each capped resource one machine has, such as `build` = 1.
    pub resources: BTreeMap<String, usize>,
}

/// Where a Box is on its machine.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct MachineInfo {
    /// The calling Box's id.
    #[serde(rename = "box")]
    box_id: i64,
    /// The machine's name.
    machine: String,
    /// The Box's slot on the machine.
    slot: u16,
    /// Add this to a repo's port for the Box's port on the machine: 10 times the slot.
    port_offset: u32,
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
        holds: Arc::new(Holds::new(config.resources)),
    };
    let http = StreamableHttpServerConfig::default()
        .with_allowed_hosts([
            "localhost",
            "127.0.0.1",
            "::1",
            config.service_host.as_str(),
        ])
        .with_legacy_session_mode(false)
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
    /// The capped resources and who holds them, shared by every call.
    holds: Arc<Holds>,
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

    /// Says which slot the Box has on its machine and how its ports shift.
    #[tool(
        description = "Where this Box is on the attached machine: its slot and port_offset. \
                       The machine is shared, so run your dev servers there on the repo's port \
                       plus port_offset; that same port number reaches them from the Box."
    )]
    async fn machine_info(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<Json<MachineInfo>, ErrorData> {
        let caller = Caller::from_headers(&parts.headers)?;
        Ok(Json(MachineInfo {
            box_id: caller.box_id.get(),
            machine: caller.machine,
            slot: caller.slot,
            port_offset: 10 * u32::from(caller.slot),
        }))
    }

    /// Runs a shell command on the machine, in the synced copy of the work tree.
    #[tool(
        description = "Run a shell command on the attached machine, in ~/moor/<box id> (call \
                       sync first), through the machine user's login shell. Returns the exit \
                       code and the last 64 KiB of combined stdout and stderr. timeout_secs \
                       defaults to 600, at most 3600; on timeout the command is killed. hold \
                       names capped resources of the machine, such as build, to hold one unit \
                       of each while the command runs; if others hold them it waits up to \
                       hold_wait_secs (default 900, at most 3600), reporting progress, and \
                       fails without running if they stay held."
    )]
    async fn exec(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ExecArgs>,
    ) -> Result<Json<ExecOutput>, ErrorData> {
        let parts = context
            .extensions
            .get::<http::request::Parts>()
            .ok_or_else(|| internal("request headers missing"))?;
        let caller = Caller::from_headers(&parts.headers)?;
        let timeout = bounded(
            "timeout_secs",
            args.timeout_secs.unwrap_or(exec::DEFAULT_TIMEOUT_SECS),
            exec::MAX_TIMEOUT_SECS,
        )?;
        let wait = args.hold_wait_secs.unwrap_or(exec::DEFAULT_HOLD_WAIT_SECS);
        if wait > exec::MAX_HOLD_WAIT_SECS {
            return Err(ErrorData::invalid_params(
                format!(
                    "hold_wait_secs must be at most {}, not {wait}",
                    exec::MAX_HOLD_WAIT_SECS
                ),
                None,
            ));
        }
        let wanted = self
            .holds
            .wanted(&args.hold)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        let token = context.meta.get_progress_token();
        let mut progress = 0.0;
        let waiting = |reason: String| {
            progress += 1.0;
            let (peer, token) = (context.peer.clone(), token.clone());
            async move {
                if let Some(token) = token {
                    let note = ProgressNotificationParam::new(token, progress).with_message(reason);
                    let _ = peer.notify_progress(note).await;
                }
            }
        };
        let run = async {
            let _held = self
                .holds
                .take(&caller, wanted, Duration::from_secs(wait), waiting)
                .await
                .map_err(|e| internal(&e))?;
            exec::run(&caller, &args.command, timeout)
                .await
                .map(Json)
                .map_err(|e| internal(&format!("could not run ssh: {e}")))
        };
        tokio::select! {
            result = run => result,
            () = context.ct.cancelled() => Err(internal("the caller went away")),
        }
    }
}

/// `secs` as a duration when it is 1 to `max`, else an error naming `name`.
fn bounded(name: &str, secs: u64, max: u64) -> Result<Duration, ErrorData> {
    if secs == 0 || secs > max {
        return Err(ErrorData::invalid_params(
            format!("{name} must be 1 to {max}, not {secs}"),
            None,
        ));
    }
    Ok(Duration::from_secs(secs))
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
