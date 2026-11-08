//! `exec_in_box`: runs a short command in one of the caller's Boxes and returns its output.

use std::io::{self, Write};

use moor_client::Client;
use moor_core::request::{ExecReq, Request as ControllerRequest};
use moor_core::view::ExecView;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::reply::Reply;
use super::{answer, enroll, target, Tools};
use crate::mcp::WRITE;
use crate::{rewrite, Person};

/// Seconds a command may run when the call names none.
const DEFAULT_TIMEOUT: u32 = 30;

/// Most seconds a command may run.
const MAX_TIMEOUT: u32 = 120;

/// Bytes of stdout, and of stderr, the tool keeps.
const OUTPUT_CAP: usize = 64 * 1024;

/// Arguments of `exec_in_box`.
#[derive(Deserialize, JsonSchema)]
struct ExecArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
    /// The command, as shell text, such as `free -m; df -h`.
    command: String,
    /// Kill the command after this many seconds: 30 by default, 120 at most.
    timeout_seconds: Option<u32>,
}

#[tool_router(router = exec_router, vis = "pub(super)")]
impl Tools {
    /// Runs a command in a Box.
    #[tool(
        description = "Run a short command in one of your Boxes and get its exit code, stdout \
                       and stderr, to see inside the Box (memory, processes, disks, docker) \
                       without asking its agent. The command is shell text, gets no stdin \
                       and no terminal, and runs beside the agent without waiting for it. \
                       An asleep Box is woken first. It is killed after timeout_seconds (30 \
                       by default, 120 at most) and timed_out says so. Each stream keeps its \
                       first 64 KiB; stdout_truncated and stderr_truncated say when more was \
                       cut. Every command leaves an exec_ran event on the Box. Needs the \
                       moor:write scope."
    )]
    async fn exec_in_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ExecArgs>,
    ) -> CallToolResult {
        let timeout = args
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(1, MAX_TIMEOUT);
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Exec(ExecReq {
                target,
                argv: vec![args.command],
                timeout_secs: Some(timeout),
            })
        });
        self.run_with(&context, WRITE, request, Reply::AsIs, send_exec)
            .await
    }
}

/// Enrolls `person`, then runs the `exec` `request` as them with no stdin; how it ended and
/// what it wrote, or why it did not run.
fn send_exec(
    controller: &Client,
    person: &Person,
    request: ControllerRequest,
    _reply: Reply,
) -> Result<Value, String> {
    enroll(controller, person)?;
    let acting = rewrite::act(request, person)?;
    let (mut stdout, mut stderr) = (Capped::default(), Capped::default());
    let answered = controller
        .pipe(acting, io::empty(), &mut stdout, &mut stderr)
        .map_err(|e| format!("the command's link to the controller failed: {e}"))?;
    answer(&answered)?;
    let view: ExecView = serde_json::from_value(answered.value.unwrap_or_default())
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

/// One output stream: its first [`OUTPUT_CAP`] bytes, and whether more came.
#[derive(Default)]
struct Capped {
    /// The bytes kept.
    kept: Vec<u8>,
    /// True once a byte past the cap arrived.
    cut: bool,
}

impl Capped {
    /// The kept bytes as text, with invalid UTF-8 replaced.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.kept).into_owned()
    }
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = OUTPUT_CAP.saturating_sub(self.kept.len());
        let take = room.min(buf.len());
        self.kept
            .extend_from_slice(buf.get(..take).unwrap_or_default());
        self.cut |= take < buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
