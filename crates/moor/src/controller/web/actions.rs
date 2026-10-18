//! State-changing endpoints: CSRF gate, body parsing and `Request` building.

use moor_core::name::BoxName;
use moor_core::request::{CloseReq, Request, StartReq, SteerReq};
use serde::Deserialize;

use super::http::Request as Http;
use super::Feed;

/// A finished HTTP reply.
pub(super) struct Reply {
    /// Status line text, e.g. `200 OK`.
    pub(super) status: &'static str,
    /// `Content-Type` value.
    pub(super) content_type: &'static str,
    /// Body text.
    pub(super) body: String,
}

impl Reply {
    /// Builds a plain-text reply.
    pub(super) fn text(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: body.into(),
        }
    }
}

/// Paths that accept POST.
pub(super) fn is_action(path: &str) -> bool {
    matches!(path, "/start" | "/steer" | "/answer" | "/close")
}

/// Runs an authenticated POST: CSRF check, body parse, then the same call a socket client makes.
pub(super) fn handle(req: &Http, feed: &dyn Feed) -> Reply {
    if let Err(why) = check_csrf(req) {
        return Reply::text("403 Forbidden", format!("forbidden: {why}"));
    }
    match build(&req.path, &req.body) {
        Err(msg) => Reply::text("400 Bad Request", msg),
        Ok(call) => match serde_json::to_string(&feed.call(call)) {
            Ok(body) => Reply {
                status: "200 OK",
                content_type: "application/json",
                body,
            },
            Err(e) => Reply::text("500 Internal Server Error", e.to_string()),
        },
    }
}

/// Rejects cross-site POSTs.
///
/// A browser cannot add a custom header to a cross-origin request without a CORS preflight,
/// and the server never answers one, so the header alone defeats forged form posts and fetches.
/// The Host, Origin and Sec-Fetch-Site checks are defence in depth.
///
/// The UI is reached through `ssh -L`, so the browser's port can differ from the bound one:
/// the port is not compared. Host must name loopback, which keeps DNS-rebinding pages (whose
/// Host is their own domain) out, and Origin, when present, must match Host exactly.
pub(super) fn check_csrf(req: &Http) -> Result<(), &'static str> {
    if req.header("x-moor-csrf") != Some("1") {
        return Err("missing X-Moor-Csrf header");
    }
    let json = req.header("content-type").is_some_and(|v| {
        v.trim()
            .to_ascii_lowercase()
            .starts_with("application/json")
    });
    if !json {
        return Err("content type must be application/json");
    }
    let Some(host) = req.header("host") else {
        return Err("missing Host header");
    };
    if !is_loopback_host(host) {
        return Err("bad host");
    }
    if req
        .header("origin")
        .is_some_and(|o| o.strip_prefix("http://") != Some(host))
    {
        return Err("bad origin");
    }
    if req
        .header("sec-fetch-site")
        .is_some_and(|v| v != "same-origin" && v != "none")
    {
        return Err("cross-site request");
    }
    Ok(())
}

/// Whether a `Host` header value names loopback, with any port.
fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix("[::1]") {
        Some(rest) => return rest.is_empty() || rest.starts_with(':'),
        None => host.split(':').next(),
    };
    matches!(name, Some("127.0.0.1" | "localhost"))
}

/// JSON body of `/start`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartBody {
    /// Repository to start the task in.
    repo: String,
    /// Task name.
    task: String,
    /// Goal text; empty means none.
    goal: String,
}

/// JSON body of `/steer`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SteerBody {
    /// Task name.
    task: String,
    /// Message to send to the task.
    text: String,
}

/// JSON body of `/answer`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerBody {
    /// Task name.
    task: String,
    /// 1-based menu option.
    option: Option<u32>,
    /// Free-text answer.
    text: Option<String>,
}

/// JSON body of `/close`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseBody {
    /// Task name.
    task: String,
}

/// Builds the core request for an action path from its JSON body.
///
/// # Errors
/// Returns a short message for bad JSON, a bad task name or an invalid option/text mix.
pub(super) fn build(path: &str, body: &[u8]) -> Result<Request, String> {
    match path {
        "/start" => {
            let b: StartBody = parse(body)?;
            Ok(Request::Start(StartReq {
                name: name(&b.task)?,
                goal: Some(b.goal).filter(|g| !g.is_empty()),
                issue: None,
                worktree: None,
                base: None,
                model: None,
                no_agent: false,
                resume: false,
                trust_moor_yaml: false,
                repo: Some(b.repo),
            }))
        }
        "/steer" => {
            let b: SteerBody = parse(body)?;
            steer(&b.task, Some(b.text), None)
        }
        "/answer" => {
            let b: AnswerBody = parse(body)?;
            match (b.option, b.text) {
                (Some(0), None) => Err("option is 1-based".into()),
                (Some(n), None) => steer(&b.task, None, Some(n)),
                (None, Some(t)) => steer(&b.task, Some(t), None),
                _ => Err("give exactly one of option or text".into()),
            }
        }
        "/close" => {
            let b: CloseBody = parse(body)?;
            Ok(Request::Close(CloseReq {
                task: name(&b.task)?,
                force: false,
                trust_moor_yaml: false,
            }))
        }
        _ => Err("unknown action".into()),
    }
}

/// Builds a steer request from text or a menu option.
fn steer(task: &str, text: Option<String>, menu: Option<u32>) -> Result<Request, String> {
    Ok(Request::Steer(SteerReq {
        task: name(task)?,
        text,
        menu,
    }))
}

/// Parses a JSON body.
fn parse<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|e| format!("bad json: {e}"))
}

/// Parses a task name.
fn name(s: &str) -> Result<BoxName, String> {
    BoxName::parse(s).map_err(|e| e.to_string())
}
