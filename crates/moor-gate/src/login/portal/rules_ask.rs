//! Writing to the agent on a repo's rules page. The person's words go to their agent place with
//! the proxy-rules skill and the rules as the page shows them, through the same ask the Tasks
//! home sends. The agent answers with the edited text, which the person reads and saves on the
//! page: the agent never saves it.

use std::net::TcpStream;

use moor_core::name::ForgeRepo;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::account::json;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// The skill the agent follows, as the Claude Code plugin ships it.
const SKILL: &str = include_str!("../../../../../agents/claude/skills/proxy-rules/SKILL.md");

/// What the page sends: the person's words and the rules they are looking at.
#[derive(Deserialize)]
pub(super) struct Asked {
    /// The repo, as `owner/name`.
    pub(super) repo: String,
    /// What the person wrote to the agent.
    pub(super) words: String,
    /// The rules text the page shows, which the agent edits.
    pub(super) text: String,
    /// The withheld requests the page lists, each as `METHOD path host`.
    #[serde(default)]
    pub(super) withheld: Vec<String>,
}

/// The skill without its front matter.
fn skill_body() -> &'static str {
    SKILL
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(SKILL, |(_, body)| body)
        .trim()
}

/// What the agent is asked: the person's words first, as the task's title, then how to answer,
/// the withheld requests, the skill, and the rules.
pub(super) fn prompt(repo: &ForgeRepo, asked: &Asked) -> String {
    let withheld = if asked.withheld.is_empty() {
        "none".to_owned()
    } else {
        asked
            .withheld
            .iter()
            .map(|w| format!("- {w}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{words}\n\n\
         This is about the Proxy rules of {repo}. Follow the proxy-rules skill below, with two \
         changes. The rules as the person sees them are at the end, so skip its step 1. Do not \
         save them: skip its step 5, since the person reads your edit on the rules page and \
         saves it there. Reply with one or two sentences naming each line you added or removed \
         and the rule it sits under, then the whole edited text in one ```yaml block, every \
         other line, comment and the order kept as they are. If the words ask for nothing you \
         can write as rules, say why and send no block.\n\n\
         Requests the rules page lists as withheld:\n{withheld}\n\n\
         <proxy-rules skill>\n{skill}\n</proxy-rules skill>\n\n\
         The rules:\n```yaml\n{text}\n```\n",
        words = asked.words.trim(),
        skill = skill_body(),
        text = asked.text.trim_end(),
    )
}

/// Sends the person's words about a repo's rules to their agent place. Answers the place that
/// took it, as a Tasks home ask does, or why it could not.
pub(super) fn ask(
    stream: &mut TcpStream,
    (gate, login): (&Gate, &Login),
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let asked = body.and_then(|b| serde_json::from_slice::<Asked>(b).ok());
    let Some(asked) = asked.filter(|a| !a.words.trim().is_empty()) else {
        let error = json!({"error": "write what the agent should change"});
        return json(stream, "400 Bad Request", &error);
    };
    let Ok(repo) = ForgeRepo::parse(asked.repo.trim()) else {
        let error = json!({"error": "name a repo as owner/name"});
        return json(stream, "400 Bad Request", &error);
    };
    if !tools::asks(&gate.controller, &session.person) {
        let error = json!({"error": "Writing to the agent needs an agent place, and moor keeps none for you. Ask the operator to add your email to agent_places."});
        return json(stream, "403 Forbidden", &error);
    }
    let origin: std::sync::Arc<str> = login.public().into();
    let words = prompt(&repo, &asked);
    match tools::ask_from_portal(
        &gate.controller,
        &session.person,
        (vec![repo.to_string()], words),
        origin,
    ) {
        Ok(sent) => {
            let name = sent.get("box").and_then(Value::as_str);
            let reply = sent.get("no_idle_place").map_or_else(
                || json!({"box": name}),
                |why| json!({"box": name, "no_idle_place": why}),
            );
            json(stream, "200 OK", &reply);
        }
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}

#[cfg(test)]
mod tests;
