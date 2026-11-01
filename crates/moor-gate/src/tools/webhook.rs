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
    /// The header the receiver wants its own token in, such as `Authorization`; its value
    /// is entered on the setup page, never here.
    header: Option<String>,
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
                       to an https URL. Pass header (such as Authorization) when the receiver \
                       wants its own token. Returns the webhook's id and a setup_url the person \
                       opens in a browser to see the key and enter the header value; nothing \
                       is delivered until then and no secret passes through here. Needs the \
                       moor:write scope."
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
            header: args.header,
        });
        let reply = Reply::WebhookAdded(self.public_url.clone());
        self.run(&context, WRITE, Ok(request), reply).await
    }

    /// Lists the caller's webhooks.
    #[tool(
        description = "List your webhooks, which receive signed POSTs of your Box events: \
                       id, url, event kinds, whether each is waiting for setup (with its \
                       setup_url) or active, and whether deliveries are working. Never shows \
                       a key. Needs the moor:read scope."
    )]
    async fn list_webhooks(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let request = ControllerRequest::WebhookList(AsReq { identity: None });
        let reply = Reply::WebhooksListed(self.public_url.clone());
        self.run(&context, READ, Ok(request), reply).await
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
