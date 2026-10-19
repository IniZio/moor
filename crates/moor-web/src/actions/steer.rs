//! Translates browser action bodies into core requests.

use moor_core::request::{CloseReq, EndSessionReq, Request, SteerReq};
use moor_core::BoxName;
use serde::Deserialize;

/// Action paths this module can build.
pub(crate) const PATHS: &[&str] = &["/steer", "/answer", "/end-session", "/close"];

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

/// JSON body of `/end-session` and `/close`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskBody {
    /// Task name.
    task: String,
}

/// Builds the core request for an action path from its JSON body.
///
/// # Errors
/// Returns a short message for bad JSON, a bad task name or an invalid option/text mix.
pub(crate) fn build(path: &str, body: &[u8]) -> Result<Request, String> {
    match path {
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
        "/end-session" => {
            let b: TaskBody = parse(body)?;
            name(&b.task)?;
            Ok(Request::EndSession(EndSessionReq { task: b.task }))
        }
        "/close" => {
            let b: TaskBody = parse(body)?;
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
    BoxName::parse_allow_bare(s).map_err(|e| e.to_string())
}
