//! The Box-facing listener: one MCP server, `moor`, with one tool, `report_step`, for the agent
//! in a Box to report the step of its workflow run.
//!
//! A Box holds no token. The shared Proxy routes `moor.moor.internal` to this listener and
//! stamps `X-Moor-Box: <box id>`, dropping any such header the Box sent. Only the Proxy reaches
//! this listener, so the header is trusted as the Box's identity and nothing else is checked.
//! The listener serves no other tool and no person's request.

use std::io;
use std::net::TcpListener;
use std::sync::Arc;

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use moor_client::{Client, ClientError};
use moor_core::request::{Request as ControllerRequest, StepReportReq};
use moor_core::response::ExitClass;
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{StepId, StepReport, StepResult};
use moor_core::BoxId;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::runtime::Runtime;

/// The header the Proxy stamps with the id of the Box that made the request.
pub const BOX_HEADER: &str = "x-moor-box";

/// The default address of the listener: loopback, so nothing but a Proxy on the host reaches it.
pub const DEFAULT_BOX_LISTEN: &str = "127.0.0.1:8444";

/// How the agent says a step went.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ResultArg {
    /// The step is done and its evidence is in `.moor/artifacts/`.
    Ok,
    /// The step cannot be done; the note says why.
    Fail,
}

/// Arguments of `report_step`.
#[derive(Deserialize, JsonSchema)]
struct ReportArgs {
    /// The id of the step you are reporting, as the step's brief names it.
    step: String,
    /// `ok` when the step is done, `fail` when it cannot be done.
    result: ResultArg,
    /// The names of the files you left in `.moor/artifacts/` as evidence. The step's brief
    /// lists the evidence it needs.
    #[serde(default)]
    evidence: Vec<String>,
    /// For `fail`, why the step cannot be done. For the last step, the run's result, in at
    /// most 2 KiB.
    #[serde(default)]
    note: String,
}

/// The tools the Box's agent gets; one copy per MCP session.
#[derive(Clone)]
struct BoxTools {
    /// Dials the controller as the gate's front.
    controller: Client,
}

#[tool_router]
impl BoxTools {
    /// Reports a step of the run in the caller's Box.
    #[tool(
        description = "Report the step of your workflow run that you were asked to do. Call it \
                       when the step is done, with result ok and the evidence files the step \
                       names left in .moor/artifacts/. Call it with result fail and a note \
                       saying why when the step cannot be done. The last step takes a note \
                       that becomes the run's result. The reply says what to do next."
    )]
    async fn report_step(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ReportArgs>,
    ) -> CallToolResult {
        let box_id = match box_of(&context) {
            Ok(id) => id,
            Err(why) => return failed(why),
        };
        let step = match StepId::parse(&args.step) {
            Ok(step) => step,
            Err(e) => return failed(e.to_string()),
        };
        let req = StepReportReq {
            box_id,
            report: StepReport {
                step,
                result: match args.result {
                    ResultArg::Ok => StepResult::Ok,
                    ResultArg::Fail => StepResult::Fail,
                },
                evidence: args.evidence,
                note: args.note,
            },
        };
        let controller = self.controller.clone();
        match tokio::task::spawn_blocking(move || send(&controller, req)).await {
            Ok(Ok(value)) if value.error => failed(value.reply),
            Ok(Ok(value)) => CallToolResult::success(vec![ContentBlock::text(value.reply)]),
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the generated handlers are async whether or not they await"
)]
#[tool_handler(router = Self::tool_router())]
impl ServerHandler for BoxTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// The Box a request came from, as the Proxy stamped it.
fn box_of(context: &RequestContext<RoleServer>) -> Result<BoxId, String> {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.headers.get(BOX_HEADER))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .map(BoxId::new)
        .ok_or_else(|| "the request does not say which Box it came from".to_owned())
}

/// A failed tool call carrying `why`.
fn failed(why: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(why)])
}

/// Sends `req` to the controller; what it tells the agent, or why it could not.
fn send(controller: &Client, req: StepReportReq) -> Result<StepReportValue, String> {
    let down = |e: &ClientError| {
        format!("moor could not be reached ({e}). Nothing is recorded; call report_step again.")
    };
    let reply = controller
        .request(ControllerRequest::StepReport(req), |_| {})
        .map_err(|e| down(&e))?;
    if reply.class != ExitClass::Ok {
        return Err(reply.error.map_or_else(
            || "moor refused the report".to_owned(),
            |e| e.message,
        ));
    }
    serde_json::from_value(reply.value.unwrap_or_default())
        .map_err(|e| format!("moor answered in a shape the gate does not know: {e}"))
}

/// What the Box-facing listener serves.
pub struct Boxes {
    /// Runs the connections.
    runtime: Runtime,
    /// The MCP service, shared by every connection so sessions survive reconnects.
    service: StreamableHttpService<BoxTools, LocalSessionManager>,
}

impl Boxes {
    /// A listener whose `report_step` reports through `controller`.
    ///
    /// # Errors
    /// The async runtime could not start.
    pub fn new(controller: Client) -> Result<Self, String> {
        // Only the Proxy reaches this listener, and a Box asks for the host the Proxy routes, so
        // the host check that guards a browser-facing server has nothing to guard here.
        let config = StreamableHttpServerConfig::default()
            .disable_allowed_hosts()
            .with_legacy_session_mode(false)
            .with_json_response(true);
        let tools = BoxTools {
            controller,
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
            .map_err(|e| format!("cannot start the Box listener's runtime: {e}"))?;
        Ok(Self { runtime, service })
    }

    /// Serves `listener` until it fails.
    ///
    /// # Errors
    /// The listener's accept failure.
    pub fn serve(&self, listener: TcpListener) -> io::Result<()> {
        listener.set_nonblocking(true)?;
        self.runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            loop {
                let (stream, _) = listener.accept().await?;
                let svc = TowerToHyperService::new(self.service.clone());
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        })
    }
}
