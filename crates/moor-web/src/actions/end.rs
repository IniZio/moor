//! Ending a whole request from the browser.
//!
//! Every child Session's open Box is closed, deepest first, and only then the request's
//! root host Session ends. Ended child Sessions are walked too: their Box can stay open.
//! If any child cannot be closed the root stays open, so the user can retry.

use std::collections::HashSet;
use std::sync::{Arc, PoisonError};

use moor_core::domain::session_state::SessionState;
use moor_core::request::{CloseReq, EndHostSessionReq, Request, ShowReq, ShowSessionReq};
use moor_core::response::ExitClass;
use moor_core::{BoxName, SessionId};
use serde_json::{json, Value};

use super::start::StartCtx;
use crate::client::Client;
use crate::http::{Handler, HttpRequest, HttpResponse, Reply};
use crate::jobs::{JobFailure, JobHandle};

#[cfg(test)]
#[path = "end_tests.rs"]
mod tests;

/// A descendant Session of the request root, ended or not.
struct Item {
    /// The Session.
    session: SessionId,
    /// Name of its Box.
    boxed: Option<String>,
    /// Id of its Box.
    box_id: Option<i64>,
    /// Whether it has ended.
    ended: bool,
}

/// Serves `POST /requests/{key}/end` and hands everything else to `fallback`.
pub fn handler(ctx: StartCtx, fallback: Handler) -> Handler {
    Arc::new(move |req: &HttpRequest| -> Reply {
        let key = if req.method == "POST" {
            req.path
                .strip_prefix("/requests/")
                .and_then(|rest| rest.strip_suffix("/end"))
        } else {
            None
        };
        key.map_or_else(|| fallback(req), |key| end(&ctx, key).into())
    })
}

/// Checks the request is open and ends it in the background.
fn end(ctx: &StartCtx, key: &str) -> HttpResponse {
    let row = ctx
        .db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .request(key);
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return HttpResponse::text("404 Not Found", "unknown request"),
        Err(e) => return HttpResponse::text("500 Internal Server Error", e.to_string()),
    };
    if row.ended_at.is_some() {
        return HttpResponse::text("409 Conflict", "request has already ended");
    }
    let ctx2 = ctx.clone();
    let job = ctx.jobs.spawn("end request", move |h| {
        run(&ctx2, h, &row.key, row.root_session)
    });
    HttpResponse {
        status: "202 Accepted",
        content_type: "application/json".into(),
        headers: Vec::new(),
        body: json!({"key": key, "job": job}).to_string().into_bytes(),
    }
}

/// Closes the descendants, ends the root and records the end.
fn run(ctx: &StartCtx, h: &JobHandle, key: &str, root: SessionId) -> Result<Value, JobFailure> {
    h.progress("listing the request's sessions");
    let mut items = Vec::new();
    let mut seen = HashSet::from([root.get()]);
    collect(&ctx.client, root, &mut seen, &mut items)?;
    h.progress("listing open boxes");
    let open = open_boxes(&ctx.client)?;
    let outcomes = close_all(&ctx.client, h, &items, &open);
    let failed: Vec<String> = outcomes
        .iter()
        .filter(|o| o.get("ok") != Some(&Value::Bool(true)))
        .map(describe)
        .collect();
    if !failed.is_empty() {
        return Err(fail(
            "close-children",
            format!(
                "{} of {} could not be closed: {}",
                failed.len(),
                outcomes.len(),
                failed.join("; ")
            ),
        ));
    }
    h.progress("ending the request session");
    let req = Request::EndHostSession(EndHostSessionReq { session: root });
    match call(&ctx.client, req) {
        Err(m) if !m.contains("has ended") => return Err(fail("end-root", m)),
        _ => {}
    }
    h.progress("saving the request");
    ctx.db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .end_request(key, (ctx.clock)())
        .map_err(|e| fail("save-request", e.to_string()))?;
    Ok(json!({"key": key, "root": root, "closed": outcomes, "root_ended": true}))
}

/// Ids of the Boxes still open.
fn open_boxes(client: &Client) -> Result<HashSet<i64>, JobFailure> {
    let req = Request::Show(ShowReq {
        box_name: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    });
    let value = call(client, req).map_err(|m| fail("list-boxes", m))?;
    let tasks = value.get("tasks").and_then(Value::as_array);
    Ok(tasks
        .into_iter()
        .flatten()
        .filter(|t| {
            !matches!(
                t.get("lifecycle").and_then(Value::as_str),
                Some("closed" | "rejected")
            )
        })
        .filter_map(|t| t.get("id").and_then(Value::as_i64))
        .collect())
}

/// Appends the descendants of `parent`, ended ones included, to `items`, deepest first.
fn collect(
    client: &Client,
    parent: SessionId,
    seen: &mut HashSet<i64>,
    items: &mut Vec<Item>,
) -> Result<(), JobFailure> {
    let req = Request::ShowSession(ShowSessionReq { session: parent });
    let value = call(client, req).map_err(|m| fail("list-children", m))?;
    let rows = value.get("children").and_then(Value::as_array);
    for row in rows.into_iter().flatten() {
        let Some(id) = row.get("session").and_then(Value::as_i64) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let session = SessionId::new(id);
        collect(client, session, seen, items)?;
        let boxed = row.get("box").and_then(Value::as_str).map(str::to_owned);
        items.push(Item {
            session,
            boxed,
            box_id: row.get("box_id").and_then(Value::as_i64),
            ended: row.get("state").and_then(Value::as_str) == Some(SessionState::Ended.as_str()),
        });
    }
    Ok(())
}

/// Closes each open Box once, by id since names get reused, and ends each live host
/// Session; keeps going past failures.
fn close_all(client: &Client, h: &JobHandle, items: &[Item], open: &HashSet<i64>) -> Vec<Value> {
    let mut closed = HashSet::new();
    let mut outcomes = Vec::new();
    for item in items {
        let request = match &item.boxed {
            Some(name) if item.box_id.is_none() => Err(format!("no id for Box {name}")),
            Some(_)
                if !item
                    .box_id
                    .is_some_and(|id| open.contains(&id) && closed.insert(id)) =>
            {
                continue
            }
            None if item.ended => continue,
            Some(name) => {
                h.progress(&format!("closing {name}"));
                BoxName::parse_allow_bare(name)
                    .map(|task| {
                        Request::Close(CloseReq {
                            box_name: task,
                            force: false,
                            trust_moor_yaml: false,
                        })
                    })
                    .map_err(|e| e.to_string())
            }
            None => {
                h.progress(&format!("ending session {}", item.session.get()));
                Ok(Request::EndHostSession(EndHostSessionReq {
                    session: item.session,
                }))
            }
        };
        let result = request.and_then(|r| call(client, r));
        let mut outcome = json!({"session": item.session, "box": item.boxed, "ok": result.is_ok()});
        if let (Err(message), Some(map)) = (result, outcome.as_object_mut()) {
            h.progress(&message);
            map.insert("message".to_owned(), Value::String(message));
        }
        outcomes.push(outcome);
    }
    outcomes
}

/// `box or session: message` for one failed outcome.
fn describe(outcome: &Value) -> String {
    let who = outcome.get("box").and_then(Value::as_str).map_or_else(
        || {
            format!(
                "session {}",
                outcome.get("session").and_then(Value::as_i64).unwrap_or(0)
            )
        },
        str::to_owned,
    );
    let message = outcome
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    format!("{who}: {message}")
}

/// Makes one socket call and returns the envelope's value or the error text.
fn call(client: &Client, request: Request) -> Result<Value, String> {
    let envelope = client.request(request, |_| {}).map_err(|e| e.to_string())?;
    if envelope.class != ExitClass::Ok {
        return Err(envelope
            .error
            .map_or_else(|| "request failed".to_owned(), |e| e.message));
    }
    Ok(envelope.value.unwrap_or(Value::Null))
}

/// A failure of `step`.
fn fail(step: &str, message: impl Into<String>) -> JobFailure {
    JobFailure {
        step: step.to_owned(),
        message: message.into(),
    }
}
