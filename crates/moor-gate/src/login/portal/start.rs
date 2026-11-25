//! Starting a Box from the portal.

use std::net::TcpStream;

use moor_core::workflow::plan_first_prompt;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::account::json;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// What a new Box asks for.
#[derive(Deserialize)]
struct NewBox {
    /// The repos, each as `owner/name`, first one first: the agent works in the first.
    repos: Vec<String>,
    /// What the agent should do.
    prompt: String,
    /// The revision to branch from; the repo's default branch when absent or empty.
    base: Option<String>,
    /// The machine to run on; the one a start picks when absent or empty.
    node: Option<String>,
    /// Whether the agent proposes a plan and waits for it to be approved before changing anything.
    #[serde(default)]
    plan_first: bool,
}

/// Starts a Box as the session's person, through the same request `start_box` sends. Answers the
/// Box's name once the Box exists, while setup carries on.
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
    let new = body.and_then(|b| serde_json::from_slice::<NewBox>(b).ok());
    let Some(new) = new.filter(|t| !t.prompt.trim().is_empty() && !t.repos.is_empty()) else {
        let error = json!({"error": "say what the agent should do, and on which repos"});
        return json(stream, "400 Bad Request", &error);
    };
    let origin: std::sync::Arc<str> = login.public().into();
    if tools::asks(&gate.controller, &session.person) {
        let repos = new.repos.iter().map(|r| r.trim().to_owned()).collect();
        let sent = tools::ask_from_portal(
            &gate.controller,
            &session.person,
            (repos, new.prompt),
            origin,
        );
        return match sent {
            Ok(asked) => {
                let name = asked.get("box").and_then(Value::as_str);
                let reply = asked.get("no_idle_place").map_or_else(
                    || json!({"box": name}),
                    |why| json!({"box": name, "no_idle_place": why}),
                );
                json(stream, "200 OK", &reply);
            }
            Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
        };
    }
    let keep = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let asked = tools::NewBox {
        repos: new.repos.iter().map(|r| r.trim().to_owned()).collect(),
        prompt: if new.plan_first {
            plan_first_prompt(&new.prompt)
        } else {
            new.prompt
        },
        base: keep(new.base),
        node: keep(new.node),
    };
    match tools::start_from_portal(&gate.controller, &session.person, asked, origin) {
        Ok(started) => {
            let name = started.pointer("/box/name").and_then(Value::as_str);
            json(stream, "200 OK", &json!({"box": name}));
        }
        // A refusal, such as missing secrets, is for the person to read.
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}
