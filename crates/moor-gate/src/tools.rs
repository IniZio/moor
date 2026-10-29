//! The MCP tools. Each maps to one request the controller already answers, sent as the
//! token's person through the same rewrite `/front` uses.

use std::path::PathBuf;

use moor_client::Client;
use moor_core::name::{BoxName, ForgeRepo};
use moor_core::request::{
    BoxTarget, CloseReq, EnrollReq, Request as ControllerRequest, ShowReq, StartReq, SteerReq,
};
use moor_core::response::{ExitClass, Response as ControllerResponse};
use moor_core::view::progress::{self, Happened, Progress};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

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
        let request = ControllerRequest::Show(ShowReq {
            target: None,
            events: None,
            agent: false,
            reports: false,
            artifacts: false,
            pane: false,
        });
        self.run(&context, READ, Ok(request), Reply::Listed).await
    }

    /// Starts a Box.
    #[tool(
        description = "Start a Box on a GitHub repo given as owner/name, optionally with a name, \
                       a first prompt for its agent and a base revision. Returns once the Box \
                       exists, while setup continues; follow the start with box_status. Pass \
                       wait=true to block until setup and the agent launch finish. Needs the \
                       moor:write scope."
    )]
    async fn start_box(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<StartArgs>,
    ) -> CallToolResult {
        let wait = args.wait.unwrap_or(false);
        let request = start_request(args);
        if wait {
            return self.run(&context, WRITE, request, Reply::Started).await;
        }
        self.run_with(&context, WRITE, request, Reply::Started, send_early)
            .await
    }

    /// Shows one Box.
    #[tool(
        description = "Show one of your Boxes: its state, branch and last activity, \
                       including why it failed (failure) or what it is waiting on (steps). \
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
                events: Some(progress::EVENTS),
                agent: true,
                reports: false,
                artifacts: false,
                pane: false,
            })
        });
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
    if send(socket, person, show.clone(), Reply::AsIs).is_ok() {
        // The Box exists already: this start attaches or is refused as busy, and either answer
        // comes from the controller, so wait for it.
        return send(socket, person, request, reply);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (path, who) = (socket.to_path_buf(), person.clone());
    std::thread::spawn(move || {
        let _ = tx.send(send(&path, &who, request, reply));
    });
    for _ in 0..EARLY_POLLS {
        match rx.recv_timeout(EARLY_POLL) {
            Ok(done) => return done,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the start did not finish".to_owned());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Ok(shown) = send(socket, person, show.clone(), reply) {
            return Ok(shown);
        }
    }
    Err("the Box did not appear in time; check box_status".to_owned())
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
    let ask = |request| {
        let acting = rewrite::act(request, person)?;
        let reply = client
            .request(acting, |_| {})
            .map_err(|e| format!("the controller is unreachable: {e}"))?;
        answer(&reply).map(|()| reply.value.unwrap_or_default())
    };
    ask(enroll)?;
    let mut value = ask(request)?;
    match reply {
        Reply::AsIs => {}
        Reply::Started => started(&mut value),
        Reply::Detailed => detailed(&mut value),
        Reply::Listed => listed(&mut value, &ask),
    }
    Ok(value)
}

/// What a tool adds to the controller's answer so a model can tell the user what is going on.
#[derive(Clone, Copy)]
enum Reply {
    /// Nothing.
    AsIs,
    /// How to follow the start.
    Started,
    /// One Box's status, failure and steps.
    Detailed,
    /// Each Box's status.
    Listed,
}

/// The raw events `box_status` keeps; the steps carry the rest.
const RAW_EVENTS: usize = 5;

/// Where a Box is, as far as what to tell a person goes.
enum Stage {
    /// It is down and the events say why.
    Failed,
    /// It is still being set up.
    Starting,
    /// Anything else; the lifecycle says it.
    Settled,
}

impl Stage {
    /// The stage of a Box in `lifecycle`.
    fn of(lifecycle: &str) -> Self {
        match lifecycle {
            "setup_failed" | "rejected" | "closed" => Self::Failed,
            "checking" | "waiting_for_box" | "setting_up" => Self::Starting,
            _ => Self::Settled,
        }
    }
}

/// The one line a person reads about a Box in `lifecycle` whose latest events are `events`.
fn status(lifecycle: &str, events: &[Happened]) -> String {
    match Stage::of(lifecycle) {
        Stage::Failed => progress::failure(events).map_or_else(
            || lifecycle.replace('_', " "),
            |why| format!("failed: {}", why.replace('\n', " ")),
        ),
        Stage::Starting => {
            let elapsed = Progress::of(events, now()).elapsed.map(progress::span);
            [
                Some("starting".to_owned()),
                elapsed,
                progress::latest(events),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
        }
        Stage::Settled => lifecycle.replace('_', " "),
    }
}

/// `value`'s events, oldest first, as the card reads them.
fn happened(value: &Value) -> Vec<Happened> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// The Box's lifecycle in a `show` row.
fn lifecycle(row: &Value) -> &str {
    row.get("lifecycle")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Adds how to follow the start to `start`'s answer.
fn started(value: &mut Value) {
    if let Some(reply) = value.as_object_mut() {
        reply.insert(
            "next".to_owned(),
            json!(
                "the machine is still starting; call box_status with this Box's name to \
                   follow it and to see why it failed, if it does"
            ),
        );
    }
}

/// Adds one Box's status, failure and steps to `show`'s answer, then trims its raw events.
fn detailed(value: &mut Value) {
    let Some(reply) = value.as_object_mut() else {
        return;
    };
    let events = happened(reply.get("events").unwrap_or(&Value::Null));
    let lifecycle = reply
        .get("box")
        .map(lifecycle)
        .unwrap_or_default()
        .to_owned();
    reply.insert("status".to_owned(), json!(status(&lifecycle, &events)));
    if matches!(Stage::of(&lifecycle), Stage::Failed) {
        reply.insert("failure".to_owned(), json!(progress::failure(&events)));
    }
    if !matches!(Stage::of(&lifecycle), Stage::Settled) {
        reply.insert(
            "steps".to_owned(),
            json!(Progress::of(&events, now()).steps),
        );
    }
    if let Some(Value::Array(raw)) = reply.get_mut("events") {
        raw.drain(..raw.len().saturating_sub(RAW_EVENTS));
    }
}

/// Adds each Box's status to `show`'s list, asking for the events of those not settled.
fn listed(value: &mut Value, ask: &dyn Fn(ControllerRequest) -> Result<Value, String>) {
    let Some(Value::Array(rows)) = value.get_mut("boxes") else {
        return;
    };
    for row in rows {
        let lifecycle = lifecycle(row).to_owned();
        let events = match (
            Stage::of(&lifecycle),
            row.get("name").and_then(Value::as_str),
        ) {
            (Stage::Settled, _) | (_, None) => Vec::new(),
            (_, Some(name)) => events_of(name, ask),
        };
        if let Some(row) = row.as_object_mut() {
            row.insert("status".to_owned(), json!(status(&lifecycle, &events)));
        }
    }
}

/// The latest events of Box `name`; none when the controller will not say.
fn events_of(
    name: &str,
    ask: &dyn Fn(ControllerRequest) -> Result<Value, String>,
) -> Vec<Happened> {
    let Ok(target) = target(name) else {
        return Vec::new();
    };
    let shown = ask(ControllerRequest::Show(ShowReq {
        target: Some(target),
        events: Some(progress::EVENTS),
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    }));
    shown.map_or_else(
        |_| Vec::new(),
        |v| happened(v.get("events").unwrap_or(&Value::Null)),
    )
}

/// Now, in unix seconds.
#[expect(
    clippy::disallowed_methods,
    reason = "the tools' one clock read; the progress it feeds takes the time as an argument"
)]
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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
