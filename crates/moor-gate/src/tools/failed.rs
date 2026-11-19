//! The tools for Boxes whose setup failed: retry one, or close them all.

use moor_client::Client;
use moor_core::name::BoxName;
use moor_core::request::{BoxReq, CloseFailedReq, Request as ControllerRequest};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::{send_early, Tools};
use crate::mcp::WRITE;
use crate::Person;

/// Re-runs setup on `name` for `person` the way `retry_box` does: the same request, answered once
/// setup has restarted, while it continues. `origin` is where a refusal sends the person for
/// missing secrets.
pub(crate) fn retry_from_portal(
    controller: &Client,
    person: &Person,
    name: BoxName,
    origin: std::sync::Arc<str>,
) -> Result<serde_json::Value, String> {
    let request = ControllerRequest::Retry(BoxReq { box_name: name });
    send_early(controller, person, request, Reply::Started(Some(origin)))
}

/// Arguments of `retry_box`.
#[derive(Deserialize, JsonSchema)]
struct RetryArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
}

/// Arguments of `close_failed_boxes`.
#[derive(Deserialize, JsonSchema)]
struct CloseFailedArgs {
    /// Close even when a Box holds unpushed commits or uncommitted files, or could not be
    /// checked for them, discarding that work. False by default, like the CLI's `--force`.
    force: Option<bool>,
}

#[tool_router(router = failed_router, vis = "pub(super)")]
impl Tools {
    /// Re-runs setup on a Box whose setup failed.
    #[tool(
        description = "Re-run setup on one of your Boxes whose setup failed (its status shows \
                       setup_failed). Refused for any other Box. Returns once setup has \
                       restarted, while it continues; follow it with box_status. Needs the \
                       moor:write scope."
    )]
    async fn retry_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RetryArgs>,
    ) -> CallToolResult {
        let request = BoxName::parse_allow_bare(&args.r#box)
            .map(|box_name| ControllerRequest::Retry(BoxReq { box_name }))
            .map_err(|e| e.to_string());
        let reply = Reply::Started(self.public_url.clone());
        self.run_with(&context, WRITE, request, reply, send_early)
            .await
    }

    /// Closes every Box whose setup failed.
    #[tool(
        description = "Close every one of your Boxes whose setup failed. Answers with the \
                       Boxes it closed and, for each it could not close, why; pass force=true \
                       to close even a Box that holds work that is not pushed. Needs the \
                       moor:write scope."
    )]
    async fn close_failed_boxes(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<CloseFailedArgs>,
    ) -> CallToolResult {
        let request = Ok(ControllerRequest::CloseFailed(CloseFailedReq {
            force: args.force.unwrap_or(false),
        }));
        self.run(&context, WRITE, request, Reply::AsIs).await
    }
}
