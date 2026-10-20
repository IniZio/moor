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
    /// Box name.
    #[serde(rename = "box")]
    box_name: String,
    /// Message to send to the Box.
    text: String,
}

/// JSON body of `/answer`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerBody {
    /// Box name.
    #[serde(rename = "box")]
    box_name: String,
    /// 1-based menu option.
    option: Option<u32>,
    /// Free-text answer.
    text: Option<String>,
}

/// JSON body of `/end-session` and `/close`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoxBody {
    /// Box name.
    #[serde(rename = "box")]
    box_name: String,
}

/// Builds the core request for an action path from its JSON body.
///
/// # Errors
/// Returns a short message for bad JSON, a bad Box name or an invalid option/text mix.
pub(crate) fn build(path: &str, body: &[u8]) -> Result<Request, String> {
    match path {
        "/steer" => {
            let b: SteerBody = parse(body)?;
            steer(&b.box_name, Some(b.text), None)
        }
        "/answer" => {
            let b: AnswerBody = parse(body)?;
            match (b.option, b.text) {
                (Some(0), None) => Err("option is 1-based".into()),
                (Some(n), None) => steer(&b.box_name, None, Some(n)),
                (None, Some(t)) => steer(&b.box_name, Some(t), None),
                _ => Err("give exactly one of option or text".into()),
            }
        }
        "/end-session" => {
            let b: BoxBody = parse(body)?;
            name(&b.box_name)?;
            Ok(Request::EndSession(EndSessionReq {
                box_name: b.box_name,
            }))
        }
        "/close" => {
            let b: BoxBody = parse(body)?;
            Ok(Request::Close(CloseReq {
                box_name: name(&b.box_name)?,
                force: false,
                trust_moor_yaml: false,
            }))
        }
        _ => Err("unknown action".into()),
    }
}

/// Builds a steer request from text or a menu option.
fn steer(box_name: &str, text: Option<String>, menu: Option<u32>) -> Result<Request, String> {
    Ok(Request::Steer(SteerReq {
        box_name: name(box_name)?,
        text,
        menu,
    }))
}

/// Parses a JSON body.
fn parse<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|e| format!("bad json: {e}"))
}

/// Parses a Box name.
fn name(s: &str) -> Result<BoxName, String> {
    BoxName::parse_allow_bare(s).map_err(|e| e.to_string())
}
