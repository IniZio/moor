//! The MCP surface: the runner's own tools beside the relayed ones, and a slotless Box refused.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{tool_handler, ErrorData, RoleServer, ServerHandler};

use crate::{caller, claim, internal, missing, Runner};

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
pub(crate) fn own_names() -> Vec<String> {
    router()
        .list_all()
        .into_iter()
        .map(|t| t.name.into_owned())
        .collect()
}
