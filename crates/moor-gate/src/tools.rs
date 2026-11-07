//! The MCP tools. Each maps to one request the controller already answers, sent as the
//! token's person through the same rewrite `/front` uses.

use std::path::PathBuf;

use moor_client::Client;
use moor_core::name::BoxName;
use moor_core::request::{
    BoxTarget, CloseReq, EnrollReq, Request as ControllerRequest, ShowReq, SteerReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::progress;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router, RoleServer};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::mcp::{Auth, READ, WRITE};
use crate::{rewrite, Person};

mod artifact;
mod events;
pub(crate) use events::{fetch, EventsArgs};
pub(crate) use listen::client_down;
mod failed;
mod inspect;
mod listen;
mod reply;
mod start;
mod webhook;
mod workflow;

use reply::Reply;
use start::start_request;

/// The tools; one copy per MCP session.
#[derive(Clone)]
pub(crate) struct Tools {
    /// The controller's front socket for the gate's namespace.
    pub(crate) front_socket: PathBuf,
    /// The gate's public origin when it serves browser sign-in, for webhook setup links.
    pub(crate) public_url: Option<std::sync::Arc<str>>,
}

/// Arguments of `start_box`.
#[derive(Deserialize, JsonSchema)]
struct StartArgs {
    /// The repo to clone, as `owner/name`.
    repo: String,
    /// More repos to clone beside `repo`, each as `owner/name` and each one you can push to;
    /// your configured `box.also` when left out, none when empty.
    also: Option<Vec<String>>,
    /// A name for the Box; one is made up when left out.
    name: Option<String>,
    /// What the Box's agent should do first.
    prompt: Option<String>,
    /// The revision to branch from; the repo's default branch when left out.
    base: Option<String>,
    /// A node you may use; omit for your default.
    node: Option<String>,
    /// Set the Box up from scratch, past any setup kept from earlier Boxes; false by default.
    #[serde(default)]
    cold: Option<bool>,
    /// Wait for setup and the agent launch to finish before returning; false by default, so
    /// the call returns once the Box exists and `box_status` shows its progress.
    wait: Option<bool>,
}

/// Arguments of the tools that name one Box.
#[derive(Deserialize, JsonSchema)]
struct BoxArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
}

/// Arguments of `close_box`.
#[derive(Deserialize, JsonSchema)]
struct CloseArgs {
    /// The Box's name, as `list_boxes` shows it.
    r#box: String,
    /// Close even when the Box holds unpushed commits or uncommitted files, or could not be
    /// checked for them, discarding that work. False by default, like the CLI's `--force`.
    force: Option<bool>,
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
    #[tool(
        description = "List your Boxes. Each carries a one-line status, including why it \
                       failed or what it is waiting on while it starts. Needs the moor:read \
                       scope."
    )]
    async fn list_boxes(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        self.run(&context, READ, Ok(show_all()), Reply::Listed)
            .await
    }

    /// Starts a Box.
    #[tool(
        description = "Start a Box on a GitHub repo given as owner/name, optionally with a name, \
                       a first prompt for its agent and a base revision. Returns once the Box \
                       exists, while setup continues; follow the start with box_status. Pass \
                       wait=true to block until setup and the agent launch finish. A refusal \
                       for missing secrets names them; the person stores them in a browser on \
                       the gate's /me/secrets page, never through an agent. Needs the \
                       moor:write scope."
    )]
    async fn start_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<StartArgs>,
    ) -> CallToolResult {
        let wait = args.wait.unwrap_or(false);
        let request = start_request(args);
        let reply = Reply::Started(self.public_url.clone());
        if wait {
            return self.run(&context, WRITE, request, reply).await;
        }
        self.run_with(&context, WRITE, request, reply, send_early)
            .await
    }

    /// Shows one Box.
    #[tool(
        description = "Show one of your Boxes: its state, branch and last activity, \
                       including why it failed (failure) or what it is waiting on (steps), \
                       the agent's last message (agent_said), its published artifacts and \
                       its reports. Needs the moor:read scope."
    )]
    async fn box_status(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<BoxArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(status_request);
        self.run(&context, READ, request, Reply::Detailed).await
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
        self.run(&context, WRITE, request, Reply::AsIs).await
    }

    /// Closes a Box.
    #[tool(
        description = "Close one of your Boxes. The controller refuses while the Box holds \
                       work that is not pushed, or when it cannot check; pass force=true to \
                       close anyway and discard that work. Needs the moor:write scope."
    )]
    async fn close_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<CloseArgs>,
    ) -> CallToolResult {
        let request = target(&args.r#box).map(|target| {
            ControllerRequest::Close(CloseReq {
                target,
                force: args.force.unwrap_or(false),
            })
        });
        self.run(&context, WRITE, request, Reply::AsIs).await
    }
}

impl Tools {
    /// Runs `request` as the caller when its token grants `scope`.
    async fn run(
        &self,
        context: &RequestContext<RoleServer>,
        scope: &str,
        request: Result<ControllerRequest, String>,
        reply: Reply,
    ) -> CallToolResult {
        self.run_with(context, scope, request, reply, send).await
    }

    /// Like [`Self::run`], sending with `sender`.
    async fn run_with(
        &self,
        context: &RequestContext<RoleServer>,
        scope: &str,
        request: Result<ControllerRequest, String>,
        reply: Reply,
        sender: SendFn,
    ) -> CallToolResult {
        let auth = match auth_for(context, scope) {
            Ok(auth) => auth,
            Err(why) => return failed(why),
        };
        let request = match request {
            Ok(request) => request,
            Err(why) => return failed(why),
        };
        let socket = self.front_socket.clone();
        let sent =
            tokio::task::spawn_blocking(move || sender(&socket, &auth.person, request, reply))
                .await;
        match sent {
            Ok(Ok(value)) => CallToolResult::structured(value),
            Ok(Err(why)) => failed(why),
            Err(e) => failed(format!("the call did not finish: {e}")),
        }
    }
}

/// The verified caller of `context`, when its token grants `scope`.
fn auth_for(context: &RequestContext<RoleServer>, scope: &str) -> Result<Auth, String> {
    let auth = context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Auth>())
        .cloned()
        .ok_or_else(|| "the request carries no verified identity".to_owned())?;
    if !auth.scopes.iter().any(|s| s == scope) {
        return Err(format!(
            "this needs the {scope} scope; sign in again with it"
        ));
    }
    Ok(auth)
}

/// The `show` of every open Box that `list_boxes` sends.
const fn show_all() -> ControllerRequest {
    ControllerRequest::Show(ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    })
}

/// The `show` of one Box that `box_status` and a resource read send.
const fn status_request(target: BoxTarget) -> ControllerRequest {
    ControllerRequest::Show(ShowReq {
        target: Some(target),
        events: Some(progress::EVENTS),
        agent: true,
        reports: true,
        artifacts: true,
        pane: false,
    })
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

/// The shape of [`send`] and [`send_early`].
type SendFn =
    fn(&std::path::Path, &Person, ControllerRequest, Reply) -> Result<serde_json::Value, String>;

/// How often [`send_early`] looks for the Box.
const EARLY_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// How many polls [`send_early`] makes (about a minute) before giving up on the Box appearing.
const EARLY_POLLS: u32 = 240;

/// Runs a `start` without waiting for it to finish: returns the Box's `show` as soon as the Box
/// exists, while the controller's start carries on in the background. A start that ends first, or for
/// a Box that exists already, answers with its own result. Other requests go through [`send`].
/// Either answer is shaped by `reply`.
fn send_early(
    socket: &std::path::Path,
    person: &Person,
    request: ControllerRequest,
    reply: Reply,
) -> Result<serde_json::Value, String> {
    let ControllerRequest::Start(start) = &request else {
        return send(socket, person, request, reply);
    };
    let show = ControllerRequest::Show(ShowReq {
        target: Some(start.name.clone().into()),
        events: Some(5),
        agent: true,
        reports: false,
        artifacts: false,
        pane: false,
    });
    if act_as(socket, person, show.clone()).is_ok() {
        // The Box exists already: this start attaches or is refused as busy, and either answer
        // comes from the controller, so wait for it.
        return send(socket, person, request, reply);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (path, who, done) = (socket.to_path_buf(), person.clone(), reply.clone());
    std::thread::spawn(move || {
        let _ = tx.send(send(&path, &who, request, done));
    });
    for _ in 0..EARLY_POLLS {
        match rx.recv_timeout(EARLY_POLL) {
            Ok(done) => return done,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the start did not finish".to_owned());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Ok(shown) = send(socket, person, show.clone(), reply.clone()) {
            return Ok(shown);
        }
    }
    Err("the Box did not appear in time; check box_status".to_owned())
}

/// Enrolls `person`, then runs `request` as them; the controller's value as is, or why not.
pub(crate) fn act_as(
    socket: &std::path::Path,
    person: &Person,
    request: ControllerRequest,
) -> Result<Value, String> {
    send(socket, person, request, Reply::AsIs)
}

/// Enrolls `person`, then runs `request` as them; the controller's value, or why not.
fn send(
    socket: &std::path::Path,
    person: &Person,
    request: ControllerRequest,
    reply: Reply,
) -> Result<Value, String> {
    let client = Client::new(socket);
    let enroll = ControllerRequest::Enroll(EnrollReq {
        identity: None,
        email: None,
    });
    let call = |request| {
        let acting = rewrite::act(request, person)?;
        client
            .request(acting, |_| {})
            .map_err(|e| format!("the controller is unreachable: {e}"))
    };
    let ask = |request| {
        let reply = call(request)?;
        answer(&reply).map(|()| reply.value.unwrap_or_default())
    };
    ask(enroll)?;
    let answered = call(request)?;
    answer(&answered).map_err(|why| reply.refused(why, &answered))?;
    let mut value = answered.value.unwrap_or_default();
    reply.apply(&mut value, &ask);
    Ok(value)
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
