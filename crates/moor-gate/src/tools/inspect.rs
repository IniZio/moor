//! `inspect_box`: a Box's pane tail, the agent's recent messages and why it looks stuck.

use moor_core::request::{InspectReq, Request as ControllerRequest};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::{target, Tools};
use crate::mcp::READ;

/// Arguments of `inspect_box`.
#[derive(Deserialize, JsonSchema)]
struct InspectArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
    /// Pane lines to return: 40 by default, 200 at most.
    lines: Option<u32>,
    /// Agent messages to return, oldest first: 5 by default, 20 at most.
    said: Option<u32>,
}

#[tool_router(router = inspect_router, vis = "pub(super)")]
impl Tools {
    /// Looks at a Box that may be stuck.
    #[tool(
        description = "Look at one of your Boxes to tell a working agent from a stuck one: \
                       the tail of its terminal (pane), the agent's recent messages (said, \
                       oldest first), its state, and `stuck` when the Box shows a sign of it \
                       (no progress for ten minutes, a refused egress, a withheld or failed \
                       credential, a rate limit, a lost link). Reads only; it never changes \
                       the Box. Needs the moor:read scope."
    )]
    async fn inspect_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<InspectArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Inspect(InspectReq {
                target,
                lines: args.lines,
                said: args.said,
            })
        });
        self.run(&context, READ, request, Reply::AsIs).await
    }
}
