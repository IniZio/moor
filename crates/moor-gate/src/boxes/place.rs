//! The tools of a claimed agent place: `exec_in_box` and `start_box`, each sent as `box-tool`
//! for the Box the Proxy named.

use moor_client::Client;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{BoxCall, BoxExecSecs, BoxToolReq, Request as ControllerRequest};
use moor_core::response::ExitClass;
use moor_core::view::ExecView;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{box_of, failed, BoxTools};
use crate::tools::exec::Capped;

/// Arguments of `exec_in_box`.
#[derive(Deserialize, JsonSchema)]
struct ExecArgs {
    /// The work Box's name, one of your user's open Boxes.
    r#box: String,
    /// The command, as shell text.
    command: String,
    /// Kill the command after this many seconds: 30 by default, 1800 at most.
    timeout_seconds: Option<u32>,
}

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
struct StartArgs {
    /// The new Box's name, such as `repo/label`.
    name: String,
    /// The repos, each `owner/name`, the Box's own first. Only your task's repos.
    repos: Vec<String>,
    /// What the new Box's agent does first.
    prompt: Option<String>,
}

#[tool_router(router = place_router, vis = "pub(super)")]
impl BoxTools {
    /// Runs a command in a work Box of the caller place's user.
    #[tool(
        description = "Run a command in one of your user's work Boxes and get its exit code, \
                       stdout and stderr. Only from an agent place a task has claimed. The \
                       command is shell text with no stdin and no terminal. It is killed after \
                       timeout_seconds (30 by default, 1800 at most) and timed_out says so. \
                       Each stream keeps its first 64 KiB; stdout_truncated and \
                       stderr_truncated say when more was cut."
    )]
    async fn exec_in_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ExecArgs>,
    ) -> CallToolResult {
        let call = exec_call(args);
        let controller = self.controller.clone();
        let sent = box_of(&context).and_then(|from| {
            call.map(|call| move || run_exec(&controller, BoxToolReq { from, call }))
        });
        match sent {
            Ok(run) => done(tokio::task::spawn_blocking(run).await),
            Err(why) => failed(why),
        }
    }

    /// Starts a Box for the caller place's user on its task's repos.
    #[tool(
        description = "Start a Box for your user on your task's repos: the first is the Box's \
                       own, the rest are cloned beside it. Only from an agent place a task has \
                       claimed. Answers once the start ends."
    )]
    async fn start_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<StartArgs>,
    ) -> CallToolResult {
        let call = start_call(args);
        let controller = self.controller.clone();
        let sent = box_of(&context).and_then(|from| {
            call.map(|call| move || run_start(&controller, BoxToolReq { from, call }))
        });
        match sent {
            Ok(run) => done(tokio::task::spawn_blocking(run).await),
            Err(why) => failed(why),
        }
    }
}

/// The `exec` call `args` ask for.
fn exec_call(args: ExecArgs) -> Result<BoxCall, String> {
    Ok(BoxCall::Exec {
        target: BoxName::parse_allow_bare(&args.r#box).map_err(|e| e.to_string())?,
        command: args.command,
        timeout: BoxExecSecs::new(args.timeout_seconds)?,
    })
}

/// The `start` call `args` ask for.
fn start_call(args: StartArgs) -> Result<BoxCall, String> {
    let mut repos = args
        .repos
        .iter()
        .map(|r| ForgeRepo::parse(r).map_err(|_| format!("{r} is not owner/name")));
    let repo = repos
        .next()
        .ok_or("name at least one repo: your task's")??;
    Ok(BoxCall::Start {
        name: BoxName::parse_allow_bare(&args.name).map_err(|e| e.to_string())?,
        repo,
        also: repos.collect::<Result<_, _>>()?,
        prompt: args.prompt,
    })
}

/// The answer of a blocking call as a tool result.
fn done(joined: Result<Result<Value, String>, tokio::task::JoinError>) -> CallToolResult {
    match joined {
        Ok(Ok(value)) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Ok(Err(why)) => failed(why),
        Err(e) => failed(format!("the call did not finish: {e}")),
    }
}

/// The controller's refusal in `reply`, if it refused.
fn refusal(reply: moor_core::Response) -> Result<Value, String> {
    if reply.class != ExitClass::Ok {
        return Err(reply
            .error
            .map_or_else(|| "moor refused".to_owned(), |e| e.message));
    }
    Ok(reply.value.unwrap_or_default())
}

/// Runs the `exec` `req`; how it ended and what it wrote.
fn run_exec(controller: &Client, req: BoxToolReq) -> Result<Value, String> {
    let (mut stdout, mut stderr) = (Capped::default(), Capped::default());
    let reply = controller
        .pipe(
            ControllerRequest::BoxTool(req),
            std::io::empty(),
            &mut stdout,
            &mut stderr,
        )
        .map_err(|e| format!("the command's link to the controller failed: {e}"))?;
    let view: ExecView = serde_json::from_value(refusal(reply)?)
        .map_err(|e| format!("the controller's answer is not an exec result: {e}"))?;
    Ok(json!({
        "exit": view.exit,
        "timed_out": view.timed_out,
        "stdout": stdout.text(),
        "stderr": stderr.text(),
        "stdout_truncated": stdout.cut,
        "stderr_truncated": stderr.cut,
    }))
}

/// Runs the `start` `req`; the controller's answer.
fn run_start(controller: &Client, req: BoxToolReq) -> Result<Value, String> {
    let reply = controller
        .request(ControllerRequest::BoxTool(req), |_| {})
        .map_err(|e| format!("moor could not be reached ({e})"))?;
    refusal(reply)
}
