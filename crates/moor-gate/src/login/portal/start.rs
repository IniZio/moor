//! Starting a task from the portal: the person's words go to an agent place, through the same
//! ask `moor ask` and the MCP `ask` tool send.

use std::net::TcpStream;

use serde::Deserialize;
use serde_json::{json, Value};

use super::super::account::json;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// What a new task asks for.
#[derive(Deserialize)]
struct NewTask {
    /// The repos, each as `owner/name`, first one first: the task's own.
    repos: Vec<String>,
    /// What the agent should do.
    prompt: String,
}

/// Starts a task as the session's person in an agent place started for it. Answers the place's
/// name once it holds the task.
pub(super) fn start(
    stream: &mut TcpStream,
    gate: &Gate,
    login: &Login,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let new = body.and_then(|b| serde_json::from_slice::<NewTask>(b).ok());
    let Some(new) = new.filter(|t| !t.prompt.trim().is_empty() && !t.repos.is_empty()) else {
        let error = json!({"error": "say what the agent should do, and on which repos"});
        return json(stream, "400 Bad Request", &error);
    };
    let origin: std::sync::Arc<str> = login.public().into();
    let repos = new.repos.iter().map(|r| r.trim().to_owned()).collect();
    match tools::ask_from_portal(
        &gate.controller,
        &session.person,
        (repos, new.prompt),
        origin,
    ) {
        Ok(asked) => {
            let name = asked.get("box").and_then(Value::as_str);
            json(stream, "200 OK", &json!({"box": name}));
        }
        // A refusal, such as missing secrets, is for the person to read.
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}
