//! The thread's host Session on the controller: the one recorded, else the live one opened
//! with the thread's key (the front may have opened it before a crash), else a new one. The
//! controller chooses the workdir and confirms the Session is live before a turn runs.

use moor_client::Client;
use moor_core::name::SessionId;
use moor_core::request::{
    BindSessionReq, OpenHostSessionReq, Request, SessionsReq, UpdateHostSessionReq,
};
use moor_core::response::{ExitClass, Response};
use moor_core::ErrorKind;
use serde_json::Value;

use crate::db::{Db, TurnRow};

/// The front name the controller's bindings know this front by.
const FRONT: &str = "slack";

/// The id of a live host Session for the thread of `row`, recording it when it is new to
/// `slack.db`. `agent` names the coordinator agent in a Session it opens. When the controller
/// refuses the Session as ended or unknown, the record is cleared and the steps run once more.
pub(super) fn ensure(
    client: &Client,
    db: &Db,
    row: &TurnRow,
    workdir: &str,
    agent: &str,
    log: &dyn Fn(&str),
) -> Result<i64, String> {
    let mut refusal = String::new();
    for _ in 0..2 {
        let id = find_or_open(client, db, row, workdir, agent, log)?;
        let Some(why) = refused(client, id, agent) else {
            return Ok(id);
        };
        db.clear_root_session(&row.channel, &row.thread_ts, id)
            .map_err(|e| e.to_string())?;
        refusal = why;
    }
    Err(refusal)
}

/// The controller's refusal of host Session `id` as not live. Any other failure leaves the
/// Session in use.
fn refused(client: &Client, id: i64, agent: &str) -> Option<String> {
    let touch = Request::UpdateHostSession(UpdateHostSessionReq {
        session: SessionId::new(id),
        agent: Some(agent.to_owned()),
        agent_session_id: None,
    });
    let error = client.request(touch, |_| {}).ok()?.error?;
    (error.kind == ErrorKind::Refused).then_some(error.message)
}

/// The key the thread's host Session is opened with: `<channel>/<thread ts>`.
pub(super) fn key(row: &TurnRow) -> String {
    format!("{}/{}", row.channel, row.thread_ts)
}

/// The recorded host Session, else the live one with the thread's key, else a new one;
/// recorded with the thread's `workdir`.
fn find_or_open(
    client: &Client,
    db: &Db,
    row: &TurnRow,
    workdir: &str,
    agent: &str,
    log: &dyn Fn(&str),
) -> Result<i64, String> {
    if let Some(id) = db
        .root_session(&row.channel, &row.thread_ts)
        .map_err(|e| e.to_string())?
    {
        return Ok(id);
    }
    let opened = call(
        client,
        Request::OpenHostSession(OpenHostSessionReq {
            workdir: None,
            agent: Some(agent.to_owned()),
            agent_session_id: None,
            parent: None,
            key: Some(key(row)),
        }),
    )?;
    let id = opened
        .get("session")
        .and_then(Value::as_i64)
        .ok_or_else(|| "the reply carries no session id".to_owned())?;
    db.set_root_session(&row.channel, &row.thread_ts, workdir, id)
        .map_err(|e| e.to_string())?;
    bind(client, row, id, log);
    Ok(id)
}

/// Binds host Session `id` to the thread's key under front `slack`, so a monitor can tell
/// which conversation started a Box. The binding only serves monitoring: a refusal is logged
/// and the turn goes on. The controller keeps the first binding of a key, so a key whose
/// earlier Session has ended stays bound to that one, and the log says so.
fn bind(client: &Client, row: &TurnRow, id: i64, log: &dyn Fn(&str)) {
    let key = key(row);
    let request = Request::BindSession(BindSessionReq {
        front: FRONT.to_owned(),
        key: key.clone(),
        session: SessionId::new(id),
        owner: FRONT.to_owned(),
    });
    match call(client, request) {
        Ok(bound) => {
            let held = bound.pointer("/binding/session").and_then(Value::as_i64);
            if held.is_some_and(|held| held != id) {
                log(&format!(
                    "host: {FRONT} {key} stays bound to host Session {} and not to {id}",
                    held.unwrap_or_default()
                ));
            }
        }
        Err(e) => log(&format!(
            "host: could not bind host Session {id} as {FRONT} {key}: {e}"
        )),
    }
}

/// The ids of the caller's live host Sessions opened with `key`.
pub(super) fn live_with_key(client: &Client, key: &str) -> Result<Vec<i64>, String> {
    let live = call(client, Request::Sessions(SessionsReq { parent: None }))?;
    Ok(live
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.get("key").and_then(Value::as_str) == Some(key))
        .filter_map(|s| s.get("session").and_then(Value::as_i64))
        .collect())
}

/// The value of the reply to `request`, or its error text.
fn call(client: &Client, request: Request) -> Result<Value, String> {
    let reply = client.request(request, |_| {}).map_err(|e| e.to_string())?;
    value(reply)
}

/// The value of a reply, or its error text.
fn value(reply: Response) -> Result<Value, String> {
    if reply.class == ExitClass::Ok {
        return Ok(reply.value.unwrap_or(Value::Null));
    }
    Err(reply
        .error
        .map_or_else(|| format!("{} failed", reply.cmd), |e| e.message))
}
