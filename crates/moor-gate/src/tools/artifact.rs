//! `get_artifact`: one Artifact a Box published, inline up to [`ARTIFACT_INLINE_MAX`] bytes.

use std::path::Path;

use base64::prelude::{Engine, BASE64_STANDARD};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{auth_for, failed, Tools};
use crate::artifact::{self, Found, Wanted};
use crate::mcp::READ;
use crate::Person;

/// The largest Artifact `get_artifact` returns inline; a larger one comes back as its record
/// and the HTTP path that downloads it.
pub(crate) const ARTIFACT_INLINE_MAX: u64 = 4 * 1024 * 1024;

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
    /// Fetches one published Artifact.
    #[tool(
        description = "Fetch an artifact one of your Boxes published, by name and optionally \
                       version (the newest when left out). An image comes back as an image, \
                       anything else as a base64 blob, with its record (name, version, \
                       content_type, size, sha256, path). Over 4 MiB only the record comes \
                       back; GET its path on this server with your token to download it. Fails \
                       when the stored bytes do not match the recorded sha256. Needs the \
                       moor:read scope."
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
        let socket = self.front_socket.clone();
        let got = tokio::task::spawn_blocking(move || fetch(&socket, &auth.person, &args)).await;
        match got {
            Ok(Ok(result)) => result,
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

/// The tool's answer for `args`, or why there is none.
fn fetch(socket: &Path, person: &Person, args: &ArtifactArgs) -> Result<CallToolResult, String> {
    let wanted = Wanted::parse(&args.r#box, &args.name, args.version).map_err(|r| text(&r))?;
    let found = artifact::find(socket, person, &wanted).map_err(|r| text(&r))?;
    let meta = record(&wanted, &found);
    if found.record.size > ARTIFACT_INLINE_MAX {
        return Ok(CallToolResult::structured(meta));
    }
    let mut bytes = Vec::new();
    artifact::read(socket, person, &found, |data| {
        bytes.extend_from_slice(data);
        Ok(())
    })
    .map_err(|r| text(&r))?;
    let data = BASE64_STANDARD.encode(&bytes);
    let content_type = &found.record.content_type;
    let block = if content_type.starts_with("image/") {
        ContentBlock::image(data, content_type)
    } else {
        let uri = format!(
            "moor://box/{}/artifacts/{}/{}",
            wanted.r#box.as_str(),
            found.record.name,
            found.record.version
        );
        ContentBlock::resource(ResourceContents::blob(data, uri).with_mime_type(content_type))
    };
    Ok(CallToolResult::success(vec![
        ContentBlock::text(meta.to_string()),
        block,
    ]))
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
