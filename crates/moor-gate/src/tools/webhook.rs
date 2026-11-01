//! The webhook tools: a signed-in user manages the webhooks that receive signed POSTs of
//! their Box events.

use moor_core::request::{AsReq, Request as ControllerRequest, WebhookAddReq, WebhookRemoveReq};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::Tools;
use crate::mcp::{READ, WRITE};

/// Arguments of `add_webhook`.
#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    /// The `https://` URL to post each event to.
    url: String,
    /// The event kinds to post; the controller's defaults when left out.
    events: Option<Vec<String>>,
}

/// Arguments of `remove_webhook`.
#[derive(Deserialize, JsonSchema)]
struct RemoveArgs {
    /// The webhook's id, as `list_webhooks` shows it.
    id: i64,
}

#[tool_router(router = webhook_router, vis = "pub(super)")]
impl Tools {
    /// Adds a webhook.
    #[tool(
        description = "Add a webhook: moor then POSTs your Box events, signed with an HMAC key, \
                       to an https URL. Returns the webhook's id and its key, shown only once. \
                       Needs the moor:write scope."
    )]
    async fn add_webhook(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<AddArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WebhookAdd(WebhookAddReq {
            identity: None,
            url: args.url,
            events: args.events,
        });
        self.run(&context, WRITE, Ok(request), Reply::WebhookAdded)
            .await
    }

    /// Lists the caller's webhooks.
    #[tool(
        description = "List your webhooks, which receive signed POSTs of your Box events: \
                       id, url, event kinds and whether deliveries are working. Never shows \
                       a key. Needs the moor:read scope."
    )]
    async fn list_webhooks(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let request = ControllerRequest::WebhookList(AsReq { identity: None });
        self.run(&context, READ, Ok(request), Reply::WebhooksListed)
            .await
    }

    /// Removes a webhook.
    #[tool(
        description = "Remove one of your webhooks by id, so its URL stops receiving your Box \
                       events. Needs the moor:write scope."
    )]
    async fn remove_webhook(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<RemoveArgs>,
    ) -> CallToolResult {
        let request = ControllerRequest::WebhookRemove(WebhookRemoveReq {
            identity: None,
            id: args.id,
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }
}
