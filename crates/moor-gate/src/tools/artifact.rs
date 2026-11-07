//! `get_artifact`: the record of one Artifact a Box published and the path that downloads it.

use moor_client::Client;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{auth_for, failed, Tools};
use crate::artifact::{self, Found, Wanted};
use crate::mcp::READ;
use crate::Person;

/// Arguments of `get_artifact`.
#[derive(Deserialize, JsonSchema)]
struct ArtifactArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
    /// The artifact's name, as `box_status` lists it under `artifacts`.
    name: String,
    /// The version to fetch; the newest when left out.
    version: Option<u32>,
}

#[tool_router(router = artifact_router, vis = "pub(super)")]
impl Tools {
    /// Describes one published Artifact.
    #[tool(
        description = "Look up an artifact one of your Boxes published, by name and optionally \
                       version (the newest when left out). Returns only its record (name, \
                       version, content_type, size, sha256, path), never the bytes, so they \
                       stay out of your context. To read the content, GET the path on this \
                       server with your token (Authorization: Bearer) and save the response \
                       to a file. Needs the moor:read scope."
    )]
    async fn get_artifact(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ArtifactArgs>,
    ) -> CallToolResult {
        let auth = match auth_for(&context, READ) {
            Ok(auth) => auth,
            Err(why) => return failed(why),
        };
        let controller = self.controller.clone();
        let got =
            tokio::task::spawn_blocking(move || fetch(&controller, &auth.person, &args)).await;
        match got {
            Ok(Ok(result)) => result,
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

/// The tool's answer for `args`, or why there is none.
fn fetch(
    controller: &Client,
    person: &Person,
    args: &ArtifactArgs,
) -> Result<CallToolResult, String> {
    let wanted = Wanted::parse(&args.r#box, &args.name, args.version).map_err(|r| text(&r))?;
    let found = artifact::find(controller, person, &wanted).map_err(|r| text(&r))?;
    Ok(CallToolResult::structured(record(&wanted, &found)))
}

/// The record of `found` as the tool shows it, with the path that downloads it.
fn record(wanted: &Wanted, found: &Found) -> Value {
    let r = &found.record;
    json!({
        "box": wanted.r#box,
        "name": r.name,
        "version": r.version,
        "content_type": r.content_type,
        "size": r.size,
        "sha256": r.sha256,
        "created_at": r.created_at,
        "path": wanted.path(r.version),
    })
}

/// A refusal's words.
fn text(refusal: &artifact::Refusal) -> String {
    refusal.message().to_owned()
}
