//! An MCP server on the host through which a Box uses one of its user's machines over ssh.
//!
//! The Box's Proxy routes `machine.moor.internal` here and names the caller in four headers:
//! the Box id, the machine's name, its ssh target and the Box's slot on it. A Box that holds no
//! slot gets the Box id and the reason instead, and every call answers that reason. `sync` mirrors the
//! Box's work tree onto the machine; `exec` runs a command there, in that copy, holding any
//! capped resources it names; `pull` brings one file back into the Box's work tree;
//! `machine_info` says which slot and ports the Box has; `machine_claim` and `machine_release`
//! take and give back the slot. The operator's `prepare`, `suspend` and `release` commands set up, pause and clean up per-Box
//! things on the machine. Relays list the tools of the machine's own stdio MCP servers beside these.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use moor_client::Client;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::{Extension, ToolCallContext};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use tokio::net::TcpListener;

use crate::beat::Progress;
use crate::caller::Caller;
use crate::exec::{ExecArgs, ExecOutput};
use crate::holds::Holds;
use crate::hooks::Hooks;
use crate::info::MachineInfo;
use crate::pull::{PullArgs, PullOutput};
use crate::relay::Relays;
use crate::sync::{SyncArgs, SyncOutput};

pub use crate::config::Config;
pub use crate::relay::Relay;

mod beat;
mod caller;
mod claim;
mod config;
mod exec;
mod holds;
mod hooks;
mod info;
mod pull;
mod relay;
mod sync;

/// Serves MCP over streamable HTTP on `listener` until accepting fails.
///
/// # Errors
/// Returns the error that stopped the listener.
pub async fn serve(listener: TcpListener, config: Config) -> io::Result<()> {
    let mirrors = config.state_dir.join("mirrors");
    let hooks = Arc::new(Hooks {
        prepare: config.prepare,
        release: config.release,
        suspend: config.suspend,
        min_free_gb: config.min_free_gb,
        client: Client::new(&config.socket),
        timeout: config.hook_timeout,
        boxes: config.state_dir.join("boxes"),
        mirrors: mirrors.clone(),
        reconcile: config.reconcile,
        listed: Mutex::default(),
        preparing: Mutex::default(),
    });
    let relays = Arc::new(Relays::new(
        config.relays,
        config.relay_idle,
        config.hook_timeout,
    ));
    let holds = Arc::new(Holds::new(config.resources));
    let runner = Runner {
        client: Client::new(&config.socket),
        mirrors,
        syncing: Arc::default(),
        holds: Arc::clone(&holds),
        hooks: Arc::clone(&hooks),
        relays: Arc::clone(&relays),
        beat: config.keep_alive,
    };
    hooks::spawn_sweep(config.sweep, runner.client.clone(), hooks, relays, holds);
    let http = StreamableHttpServerConfig::default()
        .with_allowed_hosts([
            "localhost",
            "127.0.0.1",
            "::1",
            config.service_host.as_str(),
        ])
        .with_legacy_session_mode(false)
        .with_json_response(false)
        .with_sse_keep_alive(Some(config.keep_alive));
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
    /// The operator's per-Box commands and the facts they left.
    hooks: Arc<Hooks>,
    /// The machine's stdio MCP servers and the sessions open to them.
    relays: Arc<Relays>,
    /// How often a long call reports that it still runs.
    beat: Duration,
}

#[tool_router]
impl Runner {
    /// Mirrors the calling Box's work tree onto its machine.
    #[tool(
        description = "Copy this Box's work tree to the attached machine at ~/moor/<box id>, so \
                       the machine sees your current files. .git is left out. Later syncs send \
                       only what changed, and remove files you deleted. Sync neither copies \
                       nor deletes the rsync patterns in machine_info's sync_exclude, such as \
                       node_modules, so what the machine builds for itself survives. exclude \
                       adds patterns, relative to the work tree root, such as app/build."
    )]
    async fn sync(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<SyncArgs>,
    ) -> Result<Json<SyncOutput>, ErrorData> {
        args.check()
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        let caller = caller(&context)?;
        let progress = Progress::of(&context);
        let lock = {
            let mut locks = self
                .syncing
                .lock()
                .map_err(|_| internal("sync locks poisoned"))?;
            Arc::clone(locks.entry(caller.box_id.get()).or_default())
        };
        let _held = lock.lock().await;
        self.hooks
            .before_sync(&caller)
            .await
            .map_err(|e| internal(&e))?;
        let (client, mirrors) = (self.client.clone(), self.mirrors.clone());
        let noted = caller.clone();
        let copy =
            tokio::task::spawn_blocking(move || sync::run(&client, &mirrors, &caller, &args));
        tokio::select! {
            done = copy => {
                let out = done.map_err(|e| internal(&e.to_string()))?.map_err(|e| internal(&e))?;
                self.hooks.synced(&noted).await.map_err(|e| internal(&e))?;
                Ok(Json(out))
            }
            () = progress.beat("sync", self.beat) => unreachable!("a beat never ends"),
        }
    }

    /// Copies one file from the machine into the Box's work tree.
    #[tool(
        description = "Copy one file from the attached machine into this Box's work tree, such \
                       as a screenshot a relayed tool saved there or a build log. path is \
                       absolute on the machine or relative to ~/moor/<box id>; dest is relative \
                       to your work tree. At most 32 MiB."
    )]
    async fn pull(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<PullArgs>,
    ) -> Result<Json<PullOutput>, ErrorData> {
        let caller = caller(&context)?;
        let stage = self.mirrors.with_file_name("pulls");
        let client = self.client.clone();
        pull::serve(
            client,
            stage,
            caller,
            args,
            Progress::of(&context),
            self.beat,
        )
        .await
        .map(Json)
    }

    /// Says which slot the Box has on its machine and how its ports shift.
    #[tool(
        description = "Where this Box is on the attached machine: its slot, port_offset, \
                       facts and sync_exclude. The machine is shared, so run your dev servers there on the \
                       repo's port plus port_offset; that same port number reaches them from \
                       the Box. facts are what the machine set up for this Box alone; exec \
                       sees each as env MOOR_FACT_<KEY>."
    )]
    async fn machine_info(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<Json<MachineInfo>, ErrorData> {
        let caller = Caller::from_headers(&parts.headers)?;
        let facts = self.hooks.facts(&caller).await.map_err(|e| internal(&e))?;
        let notice = self.hooks.notice(&caller);
        Ok(Json(MachineInfo {
            box_id: caller.box_id.get(),
            machine: caller.machine,
            slot: caller.slot,
            port_offset: 10 * u32::from(caller.slot),
            facts,
            sync_exclude: caller.sync_exclude,
            notice,
        }))
    }

    /// Runs a shell command on the machine, in the synced copy of the work tree.
    #[tool(
        description = "Run a shell command on the attached machine, in ~/moor/<box id> (call \
                       sync first), through the machine user's login shell, with each of \
                       machine_info's facts as env MOOR_FACT_<KEY>. Returns the exit \
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
        let caller = caller(&context)?;
        let timeout = exec::bounded(
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
        let progress = Progress::of(&context);
        let waiting = |reason: String| {
            let progress = progress.clone();
            async move { progress.note(reason).await }
        };
        let run = async {
            let facts = self.hooks.facts(&caller).await.map_err(|e| internal(&e))?;
            let prelude = hooks::state_prelude(caller.box_id.get());
            let body = format!("{prelude}cd ~/{} && {}", caller.remote_dir(), args.command);
            let script = exec::script(&hooks::fact_env(&facts), &body);
            let _held = self
                .holds
                .take(&caller, wanted, Duration::from_secs(wait), waiting)
                .await
                .map_err(|e| internal(&e))?;
            tokio::select! {
                out = exec::run(&caller.ssh, &script, timeout) => out
                    .map(Json)
                    .map_err(|e| internal(&format!("could not run ssh: {e}"))),
                () = progress.beat("exec", self.beat) => unreachable!("a beat never ends"),
            }
        };
        tokio::select! {
            result = run => result,
            () = context.ct.cancelled() => Err(internal("the caller went away")),
        }
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the generated handlers are async whether or not they await"
)]
#[tool_handler(router = router())]
impl ServerHandler for Runner {
    fn get_info(&self) -> ServerConfig {
        claim::server_config()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = router().list_all();
        if !self.relays.is_empty() && missing(&context)?.is_none() {
            let caller = caller(&context)?;
            match self.hooks.facts(&caller).await {
                Ok(facts) => {
                    let own = own_names();
                    tools.extend(self.relays.tools(&caller, &facts, &own).await);
                }
                Err(e) => eprintln!(
                    "moor-machine-runner: relays for Box {} left out: {e}",
                    caller.box_id.get()
                ),
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let claims = Runner::claim_router();
        if let Some(reason) = missing(&context)?.filter(|_| !claims.has_route(&request.name)) {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{reason}. Try again later."
            ))])
            .into());
        }
        let router = Self::tool_router() + claims;
        if router.has_route(&request.name) || self.relays.is_empty() {
            return router
                .call(ToolCallContext::new(self, request, context))
                .await;
        }
        let caller = caller(&context)?;
        let facts = self.hooks.facts(&caller).await.map_err(|e| internal(&e))?;
        let own = own_names();
        self.relays
            .call(&caller, &facts, &own, request, context)
            .await
    }
}

/// Every tool of the runner's own.
fn router() -> ToolRouter<Runner> {
    Runner::tool_router() + Runner::claim_router()
}

/// The runner's own tool names, which no relayed tool may take.
fn own_names() -> Vec<String> {
    router()
        .list_all()
        .into_iter()
        .map(|t| t.name.into_owned())
        .collect()
}

/// Why the calling Box holds no machine slot, when its Proxy says so.
fn missing(context: &RequestContext<RoleServer>) -> Result<Option<String>, ErrorData> {
    Caller::missing(&parts(context)?.headers)
}

/// The caller named in the request's headers.
fn caller(context: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
    Caller::from_headers(&parts(context)?.headers)
}

/// The request's HTTP parts, which carry the Proxy's headers.
fn parts(context: &RequestContext<RoleServer>) -> Result<&http::request::Parts, ErrorData> {
    context
        .extensions
        .get::<http::request::Parts>()
        .ok_or_else(|| internal("request headers missing"))
}

/// An MCP internal error carrying `message`.
fn internal(message: &str) -> ErrorData {
    ErrorData::internal_error(message.to_owned(), None)
}
