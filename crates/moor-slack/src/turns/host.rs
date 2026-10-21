//! The thread's host Session on the controller: the one recorded, else a live one with the
//! thread's workdir that the front opened before a crash, else a new one. The controller
//! confirms it is live before a turn runs.

use moor_client::Client;
use moor_core::name::SessionId;
use moor_core::request::{OpenHostSessionReq, Request, SessionsReq, UpdateHostSessionReq};
use moor_core::response::{ExitClass, Response};
use moor_core::ErrorKind;
use serde_json::Value;

use crate::db::{Db, TurnRow};

/// The id of a live host Session for the thread of `row`, recording it when it is new to
/// `slack.db`. `agent` names the coordinator agent in a Session it opens. When the controller
/// refuses the Session as ended or unknown, the record is cleared and the steps run once more.
pub(super) fn ensure(
    client: &Client,
    db: &Db,
    row: &TurnRow,
    workdir: &str,
    agent: &str,
) -> Result<i64, String> {
    let mut refusal = String::new();
    for _ in 0..2 {
        let id = find_or_open(client, db, row, workdir, agent)?;
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

/// The recorded host Session, else a live one with `workdir`, else a new one; recorded.
fn find_or_open(
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
    let found = live_with(client, workdir)?.into_iter().next();
    let open = || {
        let opened = call(
            client,
            Request::OpenHostSession(OpenHostSessionReq {
                workdir: workdir.to_owned(),
                agent: Some(agent.to_owned()),
                agent_session_id: None,
                parent: None,
            }),
        )?;
        let id = opened.get("session").and_then(Value::as_i64);
        id.ok_or_else(|| "the reply carries no session id".to_owned())
    };
    let id = found.map_or_else(open, Ok)?;
    db.set_root_session(&row.channel, &row.thread_ts, workdir, id)
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// The ids of the live Sessions whose workdir is `workdir`.
pub(super) fn live_with(client: &Client, workdir: &str) -> Result<Vec<i64>, String> {
    let live = call(client, Request::Sessions(SessionsReq { parent: None }))?;
    Ok(live
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.get("workdir").and_then(Value::as_str) == Some(workdir))
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
