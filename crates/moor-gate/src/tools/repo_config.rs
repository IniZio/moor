//! The repo config tools: a repo's config lives in moor, shared by every Box of the repo, and
//! anyone who can push to the repo may change it.

use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq, Request as ControllerRequest};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;

use super::reply::Reply;
use super::Tools;
use crate::mcp::{READ, WRITE};

/// Arguments of the tools that name one repo.
#[derive(Deserialize, JsonSchema)]
struct GetArgs {
    /// The repo as `owner/name`.
    repo: String,
}

/// Arguments of `set_repo_config`.
#[derive(Deserialize, JsonSchema)]
struct SetArgs {
    /// The repo as `owner/name`.
    repo: String,
    /// The whole config as YAML, in the repo config format.
    text: String,
    /// The `revision` you read it at, 0 when it has none yet; the write is refused when the
    /// stored revision differs. Leave it out to write whatever is stored.
    expected_revision: Option<u64>,
}

#[tool_router(router = repo_config_router, vis = "pub(super)")]
impl Tools {
    /// Reads a repo's config.
    #[tool(
        description = "Read a repo's config as moor stores it: the YAML text (setup, env, brief, \
                       and the rest of what a repo config holds), its revision, and who last \
                       edited it. A repo with none has revision 0 and no text. Needs the \
                       moor:read scope and a GitHub account that can see the repo."
    )]
    async fn get_repo_config(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<GetArgs>,
    ) -> CallToolResult {
        let request = repo(&args.repo).map(|repo| {
            ControllerRequest::RepoConfigGet(RepoConfigGetReq {
                identity: None,
                repo,
            })
        });
        self.run(&context, READ, request, Reply::AsIs).await
    }

    /// Replaces a repo's config.
    #[tool(
        description = "Replace a repo's config with new YAML text. It applies to every Box of \
                       the repo: env and teardown to a running Box's next command, the rest \
                       to Boxes started or rebuilt afterwards. The text must parse as \
                       a repo config, or the write is refused with the reason. Pass the revision \
                       you read as expected_revision, so a change made since is not \
                       overwritten. Needs the moor:write scope and a GitHub account that can \
                       push to the repo."
    )]
    async fn set_repo_config(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<SetArgs>,
    ) -> CallToolResult {
        let request = repo(&args.repo).map(|repo| {
            ControllerRequest::RepoConfigSet(RepoConfigSetReq {
                identity: None,
                repo,
                text: args.text,
                expected_revision: args.expected_revision,
            })
        });
        self.run(&context, WRITE, request, Reply::AsIs).await
    }
}

/// `owner/name` as a repo, or the refusal text.
fn repo(text: &str) -> Result<moor_core::name::ForgeRepo, String> {
    moor_core::name::ForgeRepo::parse(text).map_err(|e| format!("repo: {e}"))
}
