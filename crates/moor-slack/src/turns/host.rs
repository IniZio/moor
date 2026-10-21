//! The thread's host Session on the controller: the one recorded, else a live one with the
//! thread's workdir that the front opened before a crash, else a new one.

use moor_client::Client;
use moor_core::request::{OpenHostSessionReq, Request, SessionsReq};
use moor_core::response::{ExitClass, Response};
use serde_json::Value;

use crate::db::{Db, TurnRow};

/// The id of a live host Session for the thread of `row`, recording it when it is new to
/// `slack.db`. `agent` names the coordinator agent in a Session it opens.
pub(super) fn ensure(
    client: &Client,
    db: &Db,
    row: &TurnRow,
    workdir: &str,
    agent: &str,
) -> Result<i64, String> {
    if let Some(id) = db
        .root_session(&row.channel, &row.thread_ts)
        .map_err(|e| e.to_string())?
    {
        return Ok(id);
    }
    let call = |request| {
        let reply = client.request(request, |_| {}).map_err(|e| e.to_string())?;
        value(reply)
    };
    let live = call(Request::Sessions(SessionsReq { parent: None }))?;
    let found = live
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|s| s.get("workdir").and_then(Value::as_str) == Some(workdir))
        .and_then(|s| s.get("session").and_then(Value::as_i64));
    let open = || {
        let opened = call(Request::OpenHostSession(OpenHostSessionReq {
            workdir: workdir.to_owned(),
            agent: Some(agent.to_owned()),
            agent_session_id: None,
            parent: None,
        }))?;
        let id = opened.get("session").and_then(Value::as_i64);
        id.ok_or_else(|| "the reply carries no session id".to_owned())
    };
    let id = found.map_or_else(open, Ok)?;
    db.set_root_session(&row.channel, &row.thread_ts, workdir, id)
        .map_err(|e| e.to_string())?;
    Ok(id)
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
