//! The Boxes pages' data: the signed-in person's Boxes as Boxes, each with the state moor
//! derives (the page never computes one), one Box with its timeline and artifacts, and an
//! artifact's bytes. The controller decides whose Boxes a person sees, so a Box that is not
//! theirs is a 404 here, never told apart from one that does not exist.

use std::io::Write as _;
use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::progress::{PrLink, ProgressView};
use moor_core::request::{Request as ControllerRequest, ShowReq};
use moor_core::response::ExitClass;
use moor_core::view::ArtifactView;
use serde::Deserialize;
use serde_json::{json, Value};

use super::account::{failed, json, signed_out};
use super::repos::answered;
use super::routes::{read_body, Request};
use super::{Login, Session};
use crate::artifact::{self, Refusal, Wanted};
use crate::artifacts_http::Body;
use crate::{log, tools, Gate};

mod answer;
mod inbox;
mod start;

/// The events a Box page reads: the most the controller keeps in one window.
const EVENTS: u32 = 500;

/// The most an artifact download holds in memory.
const DOWNLOAD_MAX: u64 = 32 * 1024 * 1024;

/// Content types a page may show inline in an `<img>`. Never SVG: it can carry script.
const IMAGES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// The most a new Box's request body holds: its prompt, and room for the rest.
const BODY_MAX: usize = 64 * 1024;

/// Whether the request is for a Boxes route.
pub(super) fn serves(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        (
            "GET",
            "/account/api/boxes"
                | "/account/api/box"
                | "/account/api/box/artifact"
                | "/account/api/inbox"
        ) | (
            "POST",
            "/account/api/boxes" | "/account/api/box/answer" | "/account/api/inbox/read"
        )
    )
}

/// Answers a Boxes route (see [`serves`]): reads for the signed-in person, and a new Box.
pub(super) fn serve(
    stream: &mut TcpStream,
    req: &Request<'_>,
    gate: &Gate,
    login: &Login,
    session: Option<&Session>,
) {
    let (path, query) = req.target.split_once('?').unwrap_or((req.target, ""));
    // Read the body before any answer: closing on unread bytes resets the connection.
    let body = (req.method == "POST")
        .then(|| read_body(stream, req.leftover.clone(), req.length, BODY_MAX))
        .flatten();
    let Some(session) = session else {
        return signed_out(stream);
    };
    match (req.method, path) {
        ("POST", "/account/api/box/answer") => {
            answer::answer(stream, gate, login, session, req.csrf, body.as_deref());
        }
        ("POST", "/account/api/inbox/read") => {
            inbox::read(stream, gate, session, req.csrf, body.as_deref());
        }
        ("POST", _) => start::start(stream, gate, login, session, req.csrf, body.as_deref()),
        (_, "/account/api/inbox") => inbox::list(stream, gate, session),
        (_, "/account/api/boxes") => list(stream, gate, session),
        (_, "/account/api/box") => read(stream, gate, session, query),
        _ => artifact(stream, gate, session, query),
    }
}

/// A Box row as `show` sends it, the parts a page reads.
#[derive(Deserialize)]
struct Row {
    /// The Box name.
    name: String,
    /// The repo, as `OWNER/NAME`.
    repo: Option<String>,
    /// The branch.
    branch: Option<String>,
    /// The base ref.
    base: Option<String>,
    /// What the Box was asked.
    prompt: Option<String>,
    /// RFC 3339 UTC last update.
    updated_at: String,
    /// The derived state.
    progress: Option<ProgressView>,
}

/// What `show` sends, the parts a page reads.
#[derive(Deserialize)]
struct Shown {
    /// Every open Box, on a list.
    boxes: Option<Vec<Row>>,
    /// The one Box asked for.
    #[serde(rename = "box")]
    boxed: Option<Row>,
    /// The Box's events as a timeline, when asked for.
    timeline: Option<Value>,
    /// The latest version of each artifact, when asked for.
    artifacts: Option<Vec<ArtifactView>>,
}

/// The pull request's page, rebuilt from the repo the controller knows and a number.
fn pr_json(pr: Option<&PrLink>) -> Value {
    pr.map_or(Value::Null, |p| {
        json!({"repo": p.repo, "number": p.number,
               "url": format!("https://github.com/{}/pull/{}", p.repo, p.number)})
    })
}

/// A Box as a list row or the head of a Box page.
fn summary(row: &Row) -> Option<Value> {
    let progress = row.progress.as_ref()?;
    Some(json!({
        "box": row.name,
        "repo": row.repo,
        "branch": row.branch,
        "base": row.base,
        "prompt": row.prompt,
        "title": progress.title,
        "state": progress.state,
        "stale": progress.stale,
        "line": progress.line,
        "last_event_at": progress.last_event_at,
        "updated_at": row.updated_at,
        "pr": pr_json(progress.pr.as_ref()),
        "needs": progress.needs,
    }))
}

/// The value of `show`, asked as the session's person.
fn show(gate: &Gate, session: &Session, request: ShowReq) -> Result<Shown, Failure> {
    let reply = tools::ask_as(
        &gate.controller,
        &session.person,
        ControllerRequest::Show(request),
    )
    .map_err(Failure::Other)?;
    if reply.class != ExitClass::Ok {
        return Err(Failure::Reply(Box::new(reply)));
    }
    serde_json::from_value(reply.value.unwrap_or_default())
        .map_err(|e| Failure::Other(format!("unreadable answer: {e}")))
}

/// Why a read gave no value.
enum Failure {
    /// The controller said no.
    Reply(Box<moor_core::response::Response>),
    /// It could not be asked, or its answer was unreadable.
    Other(String),
}

/// Answers the person's open Boxes as Boxes.
pub(super) fn list(stream: &mut TcpStream, gate: &Gate, session: &Session) {
    let all = ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    match show(gate, session, all) {
        Ok(shown) => {
            let rows: Vec<Value> = shown.boxes.iter().flatten().filter_map(summary).collect();
            json(stream, "200 OK", &json!({"boxes": rows}));
        }
        Err(Failure::Reply(reply)) => answered(stream, &reply),
        Err(Failure::Other(why)) => failed(stream, "box list", &why),
    }
}

/// The query's `key`, decoded.
fn param(query: &str, key: &str) -> Option<String> {
    oauth2::url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// The path that serves `name` at `version` of Box `boxed`.
fn href(boxed: &str, name: &str, version: u32) -> String {
    let query = oauth2::url::form_urlencoded::Serializer::new(String::new())
        .append_pair("box", boxed)
        .append_pair("name", name)
        .append_pair("version", &version.to_string())
        .finish();
    format!("/account/api/box/artifact?{query}")
}

/// Answers one Box: its head, timeline and artifacts.
pub(super) fn read(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let Some(name) = param(query, "box").and_then(|b| BoxName::parse_allow_bare(&b).ok()) else {
        let why = json!({"error": "name a Box"});
        return json(stream, "400 Bad Request", &why);
    };
    let one = ShowReq {
        target: Some(name.into()),
        events: Some(EVENTS),
        agent: true,
        reports: false,
        artifacts: true,
        pane: false,
    };
    let shown = match show(gate, session, one) {
        Ok(shown) => shown,
        // Every Box the controller shows a person is theirs, so a refusal is no such Box.
        Err(Failure::Reply(reply)) if reply.class != ExitClass::Usage => {
            return json(stream, "404 Not Found", &json!({"error": "no such Box"}));
        }
        Err(Failure::Reply(reply)) => return answered(stream, &reply),
        Err(Failure::Other(why)) => return failed(stream, "box read", &why),
    };
    let Some(mut view) = shown.boxed.as_ref().and_then(summary) else {
        return json(stream, "404 Not Found", &json!({"error": "no such Box"}));
    };
    let boxed = shown.boxed.as_ref().map_or("", |r| r.name.as_str());
    let artifacts: Vec<Value> = shown
        .artifacts
        .iter()
        .flatten()
        .map(|a| {
            json!({"name": a.name, "version": a.version, "kind": a.kind,
                   "content_type": a.content_type, "size": a.size,
                   "created_at": a.created_at,
                   "image": IMAGES.contains(&a.content_type.as_str()),
                   "href": href(boxed, &a.name, a.version)})
        })
        .collect();
    if let Some(map) = view.as_object_mut() {
        map.insert("timeline".to_owned(), shown.timeline.unwrap_or(json!([])));
        map.insert("artifacts".to_owned(), json!(artifacts));
    }
    json(stream, "200 OK", &view);
}

/// Answers `refusal` as a JSON error.
fn refuse(stream: &mut TcpStream, refusal: &Refusal) {
    json(
        stream,
        refusal.status(),
        &json!({"error": refusal.message()}),
    );
}

/// Sends the bytes of the artifact the query names, read as the session's person: a raster image
/// inline, so a page can show it, and anything else as a download.
pub(super) fn artifact(stream: &mut TcpStream, gate: &Gate, session: &Session, query: &str) {
    let version = param(query, "version").and_then(|v| v.parse().ok());
    let wanted = Wanted::parse(
        &param(query, "box").unwrap_or_default(),
        &param(query, "name").unwrap_or_default(),
        version,
    );
    let found = wanted.and_then(|w| artifact::find(&gate.controller, &session.person, &w));
    let found = match found {
        Ok(found) => found,
        Err(refusal) => return refuse(stream, &refusal),
    };
    if found.record.size > DOWNLOAD_MAX {
        let why = Refusal::BadRequest("that artifact is too large to show here".to_owned());
        return refuse(stream, &why);
    }
    let mut body = Body {
        stream,
        record: &found.record,
        held: Vec::new(),
        started: false,
        head,
    };
    let read = artifact::read(&gate.controller, &session.person, &found, |data| {
        body.push(data)
    });
    match read {
        Ok(()) => body.finish(),
        Err(refusal) if !body.started => refuse(stream, &refusal),
        Err(refusal) => log(&format!(
            "a read of artifact {} ended short: {}",
            found.record.name,
            refusal.message()
        )),
    }
}

/// Writes the `200` head. The bytes are the agent's: an allowed image is shown, never run;
/// anything else is a download, and a browser that opens it sandboxes it.
fn head(stream: &mut TcpStream, record: &ArtifactView) -> std::io::Result<()> {
    let image = IMAGES.contains(&record.content_type.as_str());
    let (kind, disposition) = if image {
        (record.content_type.as_str(), "inline".to_owned())
    } else {
        let file: String = record
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        (
            "application/octet-stream",
            format!("attachment; filename=\"{file}\""),
        )
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Disposition: {disposition}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: sandbox\r\nConnection: close\r\n\r\n",
        record.size
    )
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::net::{TcpListener, TcpStream};

    use super::*;

    /// The head `head` writes for an artifact of `content_type` named `name`.
    fn head_of(name: &str, content_type: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let record = ArtifactView {
            name: name.to_owned(),
            version: 1,
            kind: "screenshot".to_owned(),
            content_type: content_type.to_owned(),
            size: 3,
            sha256: String::new(),
            created_at: String::new(),
        };
        head(&mut server, &record).unwrap();
        drop(server);
        let mut out = String::new();
        client.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn a_raster_image_is_shown_inline_and_never_sniffed() {
        let head = head_of("after.png", "image/png");
        assert!(head.contains("Content-Type: image/png\r\n"), "{head}");
        assert!(head.contains("Content-Disposition: inline\r\n"), "{head}");
        assert!(head.contains("X-Content-Type-Options: nosniff"), "{head}");
        assert!(head.contains("Content-Security-Policy: sandbox"), "{head}");
    }

    #[test]
    fn svg_and_everything_else_is_a_download() {
        for (name, kind) in [
            ("a.svg", "image/svg+xml"),
            ("a.html", "text/html"),
            ("a b.png", "text/plain"),
        ] {
            let head = head_of(name, kind);
            assert!(
                head.contains("Content-Type: application/octet-stream\r\n"),
                "{head}"
            );
            assert!(head.contains("attachment; filename=\"a"), "{head}");
        }
    }
}
