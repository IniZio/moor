//! The MCP tools. Each maps to one request the controller already answers, sent as the
//! token's person through the same rewrite `/front` uses.

use std::path::PathBuf;

use moor_client::Client;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{
    BoxTarget, CloseReq, EnrollReq, Request as ControllerRequest, ShowReq, StartReq, SteerReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::mcp::{Auth, READ, WRITE};
use crate::{rewrite, Person};

/// The tools; one copy per MCP session.
#[derive(Clone)]
pub(crate) struct Tools {
    /// The controller's front socket for the gate's namespace.
    pub(crate) front_socket: PathBuf,
}

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
struct StartArgs {
    /// The repo to clone, as `owner/name`.
    repo: String,
    /// A name for the Box; one is made up when left out.
    name: Option<String>,
    /// What the Box's agent should do first.
    prompt: Option<String>,
    /// The revision to branch from; the repo's default branch when left out.
    base: Option<String>,
}

/// Arguments of the tools that name one Box.
#[derive(Deserialize, JsonSchema)]
struct BoxArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
}

/// Arguments of `send_to_box`.
#[derive(Deserialize, JsonSchema)]
struct SendArgs {
    /// The Box's name.
    r#box: String,
    /// The text to type to the Box's agent.
    text: String,
}

#[tool_router]
impl Tools {
    /// Lists the caller's Boxes.
    #[tool(description = "List your Boxes with their state. Needs the moor:read scope.")]
    async fn list_boxes(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let request = ControllerRequest::Show(ShowReq {
            target: None,
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
        });
        self.run(&context, READ, Ok(request)).await
    }

    /// Starts a Box.
    #[tool(
        description = "Start a Box on a GitHub repo given as owner/name, optionally with a name, \
                       a first prompt for its agent and a base revision. Needs the moor:write \
                       scope."
    )]
    async fn start_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<StartArgs>,
    ) -> CallToolResult {
        self.run(&context, WRITE, start_request(args)).await
    }

    /// Shows one Box.
    #[tool(
        description = "Show one of your Boxes: its state, phase, branch and last activity. \
                       Needs the moor:read scope."
    )]
    async fn box_status(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<BoxArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Show(ShowReq {
                target: Some(target),
                events: Some(5),
                agent: true,
                reports: false,
                artifacts: false,
                pane: false,
            })
        });
        self.run(&context, READ, request).await
    }

    /// Types to a Box's agent.
    #[tool(description = "Send text to a Box's agent, to steer it. Needs the moor:write scope.")]
    async fn send_to_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<SendArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Steer(SteerReq {
                target,
                text: Some(args.text),
                menu: None,
                interrupt: false,
            })
        });
        self.run(&context, WRITE, request).await
    }

    /// Closes a Box.
    #[tool(
        description = "Close one of your Boxes. The controller refuses while work is \
                       unfinished. Needs the moor:write scope."
    )]
    async fn close_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<BoxArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Close(CloseReq {
                target,
                force: false,
            })
        });
        self.run(&context, WRITE, request).await
    }
}

impl Tools {
    /// Runs `request` as the caller when its token grants `scope`.
    async fn run(
        &self,
        context: &RequestContext<RoleServer>,
        scope: &str,
        request: Result<ControllerRequest, String>,
    ) -> CallToolResult {
        let auth = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Auth>())
            .cloned();
        let Some(auth) = auth else {
            return failed("the request carries no verified identity".to_owned());
        };
        if !auth.scopes.iter().any(|s| s == scope) {
            return failed(format!(
                "this tool needs the {scope} scope; sign in again with it"
            ));
        }
        let request = match request {
            Ok(request) => request,
            Err(why) => return failed(why),
        };
        let socket = self.front_socket.clone();
        let sent = tokio::task::spawn_blocking(move || send(&socket, &auth.person, request)).await;
        match sent {
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the generated handlers are async whether or not they await"
)]
#[tool_handler]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// A failed tool call carrying `why`.
fn failed(why: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(why)])
}

/// `name` as a Box target.
fn target(name: &str) -> Result<BoxTarget, String> {
    BoxName::parse_allow_bare(name)
        .map(Into::into)
        .map_err(|e| e.to_string())
}

/// The controller request that starts the Box `args` asks for.
fn start_request(args: StartArgs) -> Result<ControllerRequest, String> {
    let repo = ForgeRepo::parse(&args.repo)
        .map_err(|_| format!("{} is not a repo; give it as owner/name", args.repo))?;
    let name = match args.name {
        Some(name) => name,
        None => format!("{}/mcp-{:08x}", repo.name.to_lowercase(), random()),
    };
    let name = BoxName::parse_allow_bare(&name).map_err(|e| {
        format!("{e}; pass name as <repo>/<label> in lowercase letters, digits and dashes")
    })?;
    Ok(ControllerRequest::Start(StartReq {
        name,
        prompt: args.prompt,
        issue: None,
        base: args.base,
        model: None,
        no_agent: false,
        resume: false,
        repo: Some(repo),
        parent: None,
        identity: None,
    }))
}

/// 32 random bits, for a Box name no one chose.
fn random() -> u32 {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let bits = RandomState::new().build_hasher().finish();
    u32::try_from(bits & u64::from(u32::MAX)).unwrap_or_default()
}

/// Enrolls `person`, then runs `request` as them; the controller's value, or why not.
fn send(
    socket: &std::path::Path,
    person: &Person,
    request: ControllerRequest,
) -> Result<serde_json::Value, String> {
    let client = Client::new(socket);
    let enroll = ControllerRequest::Enroll(EnrollReq {
        identity: None,
        email: None,
    });
    let ask = |request| {
        let acting = rewrite::act(request, person)?;
        let reply = client
            .request(acting, |_| {})
            .map_err(|e| format!("the controller is unreachable: {e}"))?;
        answer(&reply).map(|()| reply.value.unwrap_or_default())
    };
    ask(enroll)?;
    ask(request)
}

/// Why the controller refused, when it did.
fn answer(reply: &ControllerResponse) -> Result<(), String> {
    if reply.class == ExitClass::Ok {
        return Ok(());
    }
    Err(reply.error.as_ref().map_or_else(
        || "the controller refused".to_owned(),
        |e| e.message.clone(),
    ))
}
