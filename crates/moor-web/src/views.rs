//! Read endpoints for the browser page: the request tree and one Box.
//!
//! Only requests that never wake a Box are sent: `sessions`, `show`, `show-session`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::request::{Request, SessionsReq, ShowReq, ShowSessionReq};
use moor_core::response::{ErrorKind, ExitClass, Response};
use moor_core::seam::forge::ForgeRepo;
use moor_core::BoxName;
use serde_json::{json, Map, Value};

use crate::db::{Db, RequestRow};
use crate::http::{Handler, HttpRequest, HttpResponse, Reply};
use moor_client::Client;

mod artifacts;
#[cfg(test)]
mod tests;

/// Number of events returned with one Box.
const BOX_EVENTS: u32 = 50;

/// Serves `GET /tree` and `/boxes/{name}`; everything else goes to `fallback`.
///
/// A request row whose root Session is no longer live is listed as a root with state `gone`.
pub fn handler(client: Client, db: Arc<Mutex<Db>>, fallback: Handler) -> Handler {
    Arc::new(move |req: &HttpRequest| -> Reply {
        if req.method != "GET" {
            return fallback(req);
        }
        let resp = match req.path.as_str() {
            "/tree" => tree(&client, &db),
            path => match path.strip_prefix("/boxes/") {
                Some(rest) => return route_box(&client, req, rest),
                None => return fallback(req),
            },
        };
        resp.into()
    })
}

/// Routes `/boxes/{name}`, `/boxes/{name}/artifacts` and `/boxes/{name}/artifacts/{file}`;
/// a Box name may itself contain a slash.
fn route_box(client: &Client, req: &HttpRequest, rest: &str) -> Reply {
    if let Some(name) = rest.strip_suffix("/artifacts") {
        return artifacts::list(client, req, name).into();
    }
    if let Some((head, file)) = rest.rsplit_once('/') {
        if let Some(name) = head.strip_suffix("/artifacts") {
            return artifacts::read(client, req, name, file);
        }
    }
    detail(client, rest).into()
}

/// A `200 OK` JSON response.
fn json_ok(body: &Value) -> HttpResponse {
    HttpResponse {
        status: "200 OK",
        content_type: "application/json".into(),
        headers: Vec::new(),
        body: body.to_string().into_bytes(),
    }
}

/// Sends `request` and returns the envelope value, or the `502` to answer with.
fn call(client: &Client, request: Request) -> Result<Value, HttpResponse> {
    let envelope = client
        .request(request, |_| {})
        .map_err(|e| HttpResponse::text("502 Bad Gateway", e.to_string()))?;
    envelope_value(envelope)
}

/// The value of an ok envelope, or the `404` or `502` to answer with.
fn envelope_value(envelope: Response) -> Result<Value, HttpResponse> {
    if envelope.class == ExitClass::Ok {
        return Ok(envelope.value.unwrap_or(Value::Null));
    }
    let mut resp = match serde_json::to_value(&envelope) {
        Ok(body) => json_ok(&body),
        Err(e) => HttpResponse::text("502 Bad Gateway", e.to_string()),
    };
    let missing = envelope
        .error
        .as_ref()
        .is_some_and(|e| e.kind == ErrorKind::NotFound);
    resp.status = if missing {
        "404 Not Found"
    } else {
        "502 Bad Gateway"
    };
    Err(resp)
}

/// A copy of `v[key]`, or null.
fn field(v: &Value, key: &str) -> Value {
    v.get(key).cloned().unwrap_or(Value::Null)
}

/// `v[key]` as an array slice.
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `v[key]` as a string.
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `GET /boxes/{name}`.
fn detail(client: &Client, name: &str) -> HttpResponse {
    let Ok(box_name) = BoxName::parse_allow_bare(name) else {
        return HttpResponse::text("400 Bad Request", "bad box name");
    };
    let show = Request::Show(ShowReq {
        target: Some(box_name.into()),
        events: Some(BOX_EVENTS),
        agent: true,
        reports: false,
        artifacts: false,
        pane: false,
    });
    match call(client, show) {
        Ok(v) => json_ok(&json!({"box": field(&v, "box"), "events": field(&v, "events")})),
        Err(resp) => resp,
    }
}

/// What the tree is built from.
struct Source<'a> {
    /// Live Sessions by parent id.
    children: HashMap<i64, Vec<&'a Value>>,
    /// Live Session ids.
    live: HashSet<i64>,
    /// Box views by Box name.
    boxes: HashMap<&'a str, &'a Value>,
    /// Request rows by root Session id.
    requests: HashMap<i64, &'a RequestRow>,
}

/// The id of a Session value.
fn id_of(session: &Value) -> i64 {
    session.get("session").and_then(Value::as_i64).unwrap_or(0)
}

/// Sort key: ordinal, then id.
fn order(session: &Value) -> (i64, i64) {
    let ordinal = session.get("ordinal").and_then(Value::as_i64);
    (ordinal.unwrap_or(0), id_of(session))
}

/// The request row as JSON.
fn request_json(row: &RequestRow) -> Value {
    json!({"key": row.key, "repo": row.repo, "created_at": row.created_at, "ended_at": row.ended_at})
}

impl Source<'_> {
    /// One Session with its descendants; `seen` stops cycles.
    fn node(&self, session: &Value, seen: &mut HashSet<i64>) -> Value {
        let id = id_of(session);
        let view = text(session, "box").and_then(|b| self.boxes.get(b).copied());
        let mut kids: Vec<&Value> = self.children.get(&id).cloned().unwrap_or_default();
        kids.sort_by_key(|k| order(k));
        let mut children = Vec::new();
        for kid in kids {
            if seen.insert(id_of(kid)) {
                children.push(self.node(kid, seen));
            }
        }
        json!({
            "session": id,
            "parent": field(session, "parent"),
            "state": field(session, "state"),
            "name": field(session, "box"),
            "workdir": field(session, "workdir"),
            "ordinal": field(session, "ordinal"),
            "request": self.requests.get(&id).map(|r| request_json(r)),
            "box": view.map(Self::box_summary),
            "agent": view.and_then(|t| t.get("agent")).map(|a| json!({
                "state": field(a, "state"),
                "question": field(a, "question"),
                "options": field(a, "options"),
            })),
            "children": children,
        })
    }

    /// The Box summary the page shows.
    fn box_summary(t: &Value) -> Value {
        let url = text(t, "repo")
            .and_then(|r| ForgeRepo::parse(r).ok())
            .map(|r| format!("https://github.com/{r}"));
        json!({
            "name": field(t, "name"),
            "repo": field(t, "repo"),
            "repo_url": url,
            "branch": field(t, "branch"),
            "lifecycle": field(t, "lifecycle"),
            "session_state": field(t, "session_state"),
            "goal": field(t, "goal"),
            "box_state": t.get("driver").map_or(Value::Null, |b| field(b, "state")),
        })
    }
}

/// `GET /tree`.
fn tree(client: &Client, db: &Mutex<Db>) -> HttpResponse {
    let sessions = call(client, Request::Sessions(SessionsReq { parent: None }));
    let show = sessions.and_then(|s| {
        let req = Request::Show(ShowReq {
            target: None,
            events: None,
            agent: true,
            reports: false,
            artifacts: false,
            pane: false,
        });
        Ok((s, call(client, req)?))
    });
    let (sessions, boxes) = match show {
        Ok(all) => all,
        Err(resp) => return resp,
    };
    let rows = db.lock().unwrap_or_else(PoisonError::into_inner).requests();
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return HttpResponse::text("500 Internal Server Error", e.to_string()),
    };
    let stranded = stranded(client, &sessions, &boxes, &rows);
    json_ok(&json!({"roots": roots(&sessions, &boxes, &rows, &stranded)}))
}

/// Whether a Box view is for a Box that is still open.
fn is_open(view: &Value) -> bool {
    !matches!(text(view, "lifecycle"), Some("closed" | "rejected"))
}

/// Ended child Sessions of open requests whose Box is still open and has no live Session.
fn stranded(client: &Client, sessions: &Value, boxes: &Value, rows: &[RequestRow]) -> Vec<Value> {
    let open: HashSet<i64> = items(boxes, "boxes")
        .iter()
        .filter(|t| is_open(t))
        .filter_map(|t| t.get("id").and_then(Value::as_i64))
        .collect();
    let mut hidden: HashSet<i64> = items(sessions, "sessions")
        .iter()
        .filter_map(|s| s.get("box_id").and_then(Value::as_i64))
        .collect();
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.ended_at.is_none()) {
        let root = row.root_session.get();
        let mut seen = HashSet::from([root]);
        let mut walk = vec![root];
        while let Some(parent) = walk.pop() {
            let req = Request::ShowSession(ShowSessionReq {
                session: moor_core::SessionId::new(parent),
            });
            let Ok(value) = call(client, req) else {
                continue;
            };
            for kid in items(&value, "children") {
                let id = id_of(kid);
                if !seen.insert(id) {
                    continue;
                }
                walk.push(id);
                let box_id = kid.get("box_id").and_then(Value::as_i64);
                if text(kid, "state") == Some("ended")
                    && box_id.is_some_and(|b| open.contains(&b) && hidden.insert(b))
                {
                    out.push(
                        json!({"session": id, "box": field(kid, "box"), "parent": root,
                        "state": "ended", "workdir": null, "ordinal": null}),
                    );
                }
            }
        }
    }
    out
}

/// The root nodes: live roots, then request rows that are no longer live.
fn roots(sessions: &Value, boxes: &Value, rows: &[RequestRow], stranded: &[Value]) -> Vec<Value> {
    let all = items(sessions, "sessions");
    let live: HashSet<i64> = all.iter().map(id_of).collect();
    let mut children: HashMap<i64, Vec<&Value>> = HashMap::new();
    let mut tops: Vec<&Value> = Vec::new();
    for s in all {
        match s.get("parent").and_then(Value::as_i64) {
            Some(p) if live.contains(&p) => children.entry(p).or_default().push(s),
            _ => tops.push(s),
        }
    }
    tops.sort_by_key(|s| order(s));
    for s in stranded {
        let parent = s.get("parent").and_then(Value::as_i64).unwrap_or(0);
        children.entry(parent).or_default().push(s);
    }
    let src = Source {
        children,
        live,
        boxes: items(boxes, "boxes")
            .iter()
            .filter(|t| is_open(t))
            .filter_map(|t| Some((text(t, "name")?, t)))
            .collect(),
        requests: rows.iter().map(|r| (r.root_session.get(), r)).collect(),
    };
    let mut seen: HashSet<i64> = tops.iter().map(|s| id_of(s)).collect();
    let mut out: Vec<Value> = tops.iter().map(|s| src.node(s, &mut seen)).collect();
    out.extend(
        rows.iter()
            .filter(|r| !src.live.contains(&r.root_session.get()))
            .map(|r| gone(r, &src, &mut seen)),
    );
    out
}

/// A root for a request whose Session is no longer live.
fn gone(row: &RequestRow, src: &Source, seen: &mut HashSet<i64>) -> Value {
    let mut node = Map::new();
    node.insert("session".into(), json!(row.root_session.get()));
    for key in ["parent", "name", "workdir", "ordinal", "box", "agent"] {
        node.insert(key.into(), Value::Null);
    }
    node.insert("state".into(), json!("gone"));
    node.insert("request".into(), request_json(row));
    let mut kids: Vec<&Value> = src
        .children
        .get(&row.root_session.get())
        .cloned()
        .unwrap_or_default();
    kids.sort_by_key(|k| order(k));
    let mut nodes = Vec::new();
    for kid in kids {
        if seen.insert(id_of(kid)) {
            nodes.push(src.node(kid, seen));
        }
    }
    let kids = nodes;
    node.insert("children".into(), json!(kids));
    Value::Object(node)
}
