//! The tools of a claimed agent place: `exec_in_box`, `exec_wait`, `start_box` and `close_box`,
//! each sent as `box-tool` for the Box the Proxy named.

use moor_client::Client;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{
    BoxCall, BoxExecSecs, BoxRunId, BoxToolReq, Request as ControllerRequest,
};
use moor_core::response::ExitClass;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{box_of, failed, BoxTools};

/// Arguments of `exec_in_box`.
#[derive(Deserialize, JsonSchema)]
struct ExecArgs {
    /// The work Box's name, one this task started and still open.
    r#box: String,
    /// The command, as shell text.
    command: String,
    /// Kill the command after this many seconds: 30 by default, 1800 at most. Set it for a
    /// build or a test run.
    timeout_seconds: Option<u32>,
}

/// Arguments of `exec_wait`.
#[derive(Deserialize, JsonSchema)]
struct WaitArgs {
    /// The run id of an `exec_in_box` answer whose state was running.
    run: u32,
}

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
struct StartArgs {
    /// The new Box's name, such as `repo/label`.
    name: String,
    /// The repos, each `owner/name`, the Box's own first. Only your task's repos.
    repos: Vec<String>,
    /// Skip the repo's setup lines, so the Box is ready once the repos are cloned; you install
    /// what you need with `exec_in_box`. False by default.
    no_setup: Option<bool>,
}

/// Arguments of `close_box`.
#[derive(Deserialize, JsonSchema)]
struct CloseArgs {
    /// The work Box's name, one this task started and still open.
    r#box: String,
}

#[tool_router(router = place_router, vis = "pub(super)")]
impl BoxTools {
    /// Runs a command in a work Box the caller place started.
    #[tool(
        description = "Run a command in a work Box this task started and get its exit code, \
                       stdout and stderr. Only from an agent place a task has claimed. The \
                       command is shell text with no stdin and no terminal. It is killed after \
                       timeout_seconds (30 by default, 1800 at most; set it for builds and \
                       tests). The answer comes within about 45 s with the run id, the output \
                       so far and its state: exited with the exit code, timed_out, stopped, \
                       or running. While it is running, call exec_wait with that run. Each \
                       answer has the output since the last one. Each stream \
                       keeps its first 64 KiB; stdout_truncated and stderr_truncated say when \
                       more was cut."
    )]
    async fn exec_in_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ExecArgs>,
    ) -> CallToolResult {
        let call = exec_call(args);
        let controller = self.controller.clone();
        let sent = box_of(&context).and_then(|from| {
            call.map(|call| move || run_answered(&controller, BoxToolReq { from, call }))
        });
        match sent {
            Ok(run) => done(tokio::task::spawn_blocking(run).await),
            Err(why) => failed(why),
        }
    }

    /// Waits on a command of the caller place that `exec_in_box` left running.
    #[tool(
        description = "Wait on a command that exec_in_box left running. Answers within about \
                       45 s with the output since the last answer and the command's state, as \
                       exec_in_box gives it. Call it again while the state is running. Only from an agent place a task has claimed, on this \
                       task's own runs."
    )]
    async fn exec_wait(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<WaitArgs>,
    ) -> CallToolResult {
        let call = BoxCall::Wait {
            run: BoxRunId(args.run),
        };
        let controller = self.controller.clone();
        match box_of(&context) {
            Ok(from) => done(
                tokio::task::spawn_blocking(move || {
                    run_answered(&controller, BoxToolReq { from, call })
                })
                .await,
            ),
            Err(why) => failed(why),
        }
    }

    /// Starts a Box for the caller place's user on its task's repos.
    #[tool(
        description = "Start a Box for your user on your task's repos: the first is the Box's \
                       own, the rest are cloned beside it. The Box runs no agent: run commands in it with \
                       exec_in_box. Set no_setup to skip the repo's setup lines when the work \
                       does not need them. Only from an agent place a task has claimed. \
                       Answers once the start ends."
    )]
    async fn start_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<StartArgs>,
    ) -> CallToolResult {
        let call = start_call(&args);
        let controller = self.controller.clone();
        let sent = box_of(&context).and_then(|from| {
            call.map(|call| move || run_answered(&controller, BoxToolReq { from, call }))
        });
        match sent {
            Ok(run) => done(tokio::task::spawn_blocking(run).await),
            Err(why) => failed(why),
        }
    }

    /// Closes a work Box the caller place started.
    #[tool(
        description = "Close a work Box this task started when its work is done. Only from an \
                       agent place a task has claimed. It is refused while the Box holds work \
                       that is not pushed: push it with exec_in_box, then close again. There is \
                       no force. Answers once the Box is closed."
    )]
    async fn close_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<CloseArgs>,
    ) -> CallToolResult {
        let call = close_call(&args);
        let controller = self.controller.clone();
        let sent = box_of(&context).and_then(|from| {
            call.map(|call| move || run_answered(&controller, BoxToolReq { from, call }))
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
fn start_call(args: &StartArgs) -> Result<BoxCall, String> {
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
        no_setup: args.no_setup.unwrap_or(false),
    })
}

/// The `close` call `args` ask for.
fn close_call(args: &CloseArgs) -> Result<BoxCall, String> {
    Ok(BoxCall::Close {
        target: BoxName::parse_allow_bare(&args.r#box).map_err(|e| e.to_string())?,
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

/// Runs `req`; the controller's answer.
fn run_answered(controller: &Client, req: BoxToolReq) -> Result<Value, String> {
    let reply = controller
        .request(ControllerRequest::BoxTool(req), |_| {})
        .map_err(|e| format!("moor could not be reached ({e})"))?;
    refusal(reply)
}
