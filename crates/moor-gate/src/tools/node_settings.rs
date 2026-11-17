//! The node settings tools: a node's cap and labels live in moor, and its owner changes them.

use std::num::NonZeroU32;

use moor_core::request::{CapEdit, NodeSettingsGetReq, NodeSettingsSetReq, Request};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::Tools;
use crate::mcp::{READ, WRITE};

/// Arguments of `get_node_settings`.
#[derive(Deserialize, JsonSchema)]
struct GetArgs {
    /// The node's name, as `list_nodes` shows it.
    name: String,
}

/// Arguments of `set_node_settings`.
#[derive(Deserialize, JsonSchema)]
struct SetArgs {
    /// The node's name, as `list_nodes` shows it.
    name: String,
    /// The most live Boxes the node holds, at least 1. Leave it out to keep the cap as it is.
    max_boxes: Option<NonZeroU32>,
    /// True to remove the cap, so free memory alone decides. Not with `max_boxes`.
    no_cap: Option<bool>,
    /// The node's whole label list. Leave it out to keep the labels as they are.
    labels: Option<Vec<String>>,
    /// The `revision` you read, 0 when the settings were never written; the write is refused when
    /// the stored revision differs. Leave it out to write over whatever is stored.
    expected_revision: Option<u64>,
}

#[tool_router(router = node_settings_router, vis = "pub(super)")]
impl Tools {
    /// Reads a node's settings.
    #[tool(
        description = "Read a node's settings as moor stores them: max_boxes (none means no \
                       cap, so free memory decides), its labels, its revision, who last \
                       edited them, and can_edit. Needs the moor:read scope."
    )]
    async fn get_node_settings(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<GetArgs>,
    ) -> CallToolResult {
        let request = Request::NodeSettingsGet(NodeSettingsGetReq {
            identity: None,
            name: args.name,
        });
        self.run(&context, READ, Ok(request), Reply::AsIs).await
    }

    /// Changes a node's cap or labels.
    #[tool(
        description = "Change a node's cap or labels: max_boxes, or no_cap to remove it, and \
                       labels as the whole list. The next Box start reads them, and Boxes \
                       waiting for room on the node start when the change makes room. You \
                       change your own node; only the operator changes a shared one. Pass \
                       the revision you read as expected_revision, so a change made since is \
                       not overwritten. Needs the moor:write scope."
    )]
    async fn set_node_settings(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<SetArgs>,
    ) -> CallToolResult {
        let max_boxes = match (args.max_boxes, args.no_cap) {
            (Some(_), Some(true)) => {
                let both = "max_boxes and no_cap together mean nothing".to_owned();
                return self.run(&context, WRITE, Err(both), Reply::AsIs).await;
            }
            (Some(n), _) => CapEdit::Set(n),
            (None, Some(true)) => CapEdit::Clear,
            (None, _) => CapEdit::Keep,
        };
        let request = Request::NodeSettingsSet(NodeSettingsSetReq {
            identity: None,
            name: args.name,
            max_boxes,
            labels: args.labels,
            expected_revision: args.expected_revision,
        });
        self.run(&context, WRITE, Ok(request), Reply::AsIs).await
    }
}
