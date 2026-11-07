//! `GET /events`: `list_events` over plain HTTP, for a bot that polls with curl and an Authgear
//! access token. It authenticates like `/mcp` and reads through the same fetch the tool uses.

use std::io::Write;
use std::net::TcpStream;
use std::str::FromStr;

use moor_core::name::BoxName;
use serde_json::json;

use crate::http::header;
use crate::mcp::READ;
use crate::streams::Streams;
use crate::tools::{fetch, EventsArgs};
use crate::{log, Gate, Mcp, Person};

/// Answers one `GET /events` whose request line target is `target`.
pub(crate) fn serve(
    mut stream: TcpStream,
    target: &str,
    req: &httparse::Request<'_, '_>,
    gate: &Gate,
    streams: &Streams,
) {
    let Some((mcp, person)) = reader(&mut stream, req, gate, "/events") else {
        return;
    };
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let args = match parse(query) {
        Ok(args) => args,
        Err(why) => return reply(&mut stream, "400 Bad Request", None, &error(&why)),
    };
    let controller = mcp.controller();
    let _held = streams.hold(&stream);
    match fetch(controller, &person, args) {
        Ok(doc) => reply(&mut stream, "200 OK", None, &doc.to_string()),
        Err(why) if why.down => crate::http::unavailable(&mut stream),
        Err(why) => reply(&mut stream, "502 Bad Gateway", None, &error(&why.why)),
    }
}

/// Who a request to `route` speaks for when its token grants `moor:read`; otherwise answers
/// 401 or 403 and gives none.
pub(crate) fn reader<'g>(
    stream: &mut TcpStream,
    req: &httparse::Request<'_, '_>,
    gate: &'g Gate,
    route: &str,
) -> Option<(&'g Mcp, Person)> {
    let mcp = gate.mcp.as_ref()?;
    let authorization = header(req, "authorization");
    let (person, scopes) = match mcp.authenticate(authorization, gate) {
        Ok(who) => who,
        Err(why) => {
            if authorization.is_some() {
                log(&format!("{route} refused a token: {why}"));
            }
            let challenge = mcp.challenge();
            reply(stream, "401 Unauthorized", Some(&challenge), &error(&why));
            return None;
        }
    };
    if !scopes.iter().any(|s| s == READ) {
        let why = format!("this needs the {READ} scope");
        reply(stream, "403 Forbidden", None, &error(&why));
        return None;
    }
    Some((mcp, person))
}

/// The query's parameters as `list_events` arguments, or the parameter that is wrong.
fn parse(query: &str) -> Result<EventsArgs, String> {
    let mut args = EventsArgs {
        after: None,
        kinds: None,
        r#box: None,
        limit: None,
        wait_seconds: None,
    };
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "after" => args.after = Some(number("after", &value)?),
            "limit" => args.limit = Some(number("limit", &value)?),
            "wait_seconds" => args.wait_seconds = Some(number("wait_seconds", &value)?),
            "kind" => args
                .kinds
                .get_or_insert_with(Vec::new)
                .push(value.into_owned()),
            "box" => {
                BoxName::parse_allow_bare(&value).map_err(|e| format!("box: {e}"))?;
                args.r#box = Some(value.into_owned());
            }
            _ => {}
        }
    }
    Ok(args)
}

/// `value` as a whole number, or an error naming `name`.
fn number<T: FromStr>(name: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{name} must be a whole number, not {value:?}"))
}

/// The JSON body of an error.
pub(crate) fn error(why: &str) -> String {
    json!({ "error": why }).to_string()
}

/// Writes a whole JSON response, never cached, and lets the connection close.
pub(crate) fn reply(stream: &mut TcpStream, status: &str, challenge: Option<&str>, body: &str) {
    let extra = challenge.map_or_else(String::new, |c| format!("WWW-Authenticate: {c}\r\n"));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
