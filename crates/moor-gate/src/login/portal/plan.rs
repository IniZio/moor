//! A plan made by an agent from the portal: a short Box that drafts a workflow's plan.

use std::net::TcpStream;

use moor_core::workflow::draft_prompt;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::account::json;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// What a draft is made from.
#[derive(Deserialize)]
struct Draft {
    /// The workflow's repos, each `owner/name`, first one first.
    repos: Vec<String>,
    /// The workflow prompt the plan is for.
    prompt: String,
}

/// Answers `403` unless the request carries the session's CSRF token.
fn forbidden(stream: &mut TcpStream, session: &Session, csrf: Option<&str>) -> bool {
    if csrf.is_some_and(|c| check_csrf(session, c)) {
        return false;
    }
    let error = json!({"error": "write from the page you are signed in on"});
    json(stream, "403 Forbidden", &error);
    true
}

/// The Box a draft starts: the workflow's repos, asked to leave the prompt's plan as evidence.
fn asked(body: Option<&[u8]>) -> Option<tools::NewBox> {
    let draft: Draft = serde_json::from_slice(body?).ok()?;
    let repos: Vec<String> = draft.repos.iter().map(|r| r.trim().to_owned()).collect();
    (!draft.prompt.trim().is_empty() && !repos.is_empty()).then(|| tools::NewBox {
        repos,
        prompt: draft_prompt(&draft.prompt),
        base: None,
        node: None,
    })
}

/// Starts a short Box as the session's person that drafts a plan for a workflow prompt. Answers
/// the Box's name; the page reads the plan from its evidence.
pub(super) fn draft(
    stream: &mut TcpStream,
    gate: &Gate,
    login: &Login,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if forbidden(stream, session, csrf) {
        return;
    }
    let Some(new) = asked(body) else {
        let error = json!({"error": "write the prompt and pick a repo first"});
        return json(stream, "400 Bad Request", &error);
    };
    let origin: std::sync::Arc<str> = login.public().into();
    match tools::start_from_portal(&gate.controller, &session.person, new, origin) {
        Ok(started) => {
            let name = started.pointer("/box/name").and_then(Value::as_str);
            json(stream, "200 OK", &json!({"box": name}));
        }
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}

#[cfg(test)]
mod tests {
    use super::asked;

    #[test]
    fn a_draft_asks_for_the_prompts_plan_in_the_workflows_repos() {
        let new = asked(Some(
            br#"{"repos": [" o/a ", "o/b"], "prompt": "Bump deps."}"#,
        ))
        .unwrap();
        assert_eq!(new.repos, ["o/a", "o/b"]);
        assert!(new.prompt.contains("Bump deps.") && new.prompt.contains("plan.json"));
        for bad in [
            r#"{"repos": [], "prompt": "x"}"#,
            r#"{"repos": ["o/a"], "prompt": " "}"#,
            "x",
        ] {
            assert!(asked(Some(bad.as_bytes())).is_none(), "{bad}");
        }
    }
}
