//! Steering a Box's agent from the portal: a message after its current tool call, or an
//! interrupt that stops its turn first.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{Request as ControllerRequest, SteerReq};
use moor_core::response::{ExitClass, Response};
use serde::Deserialize;
use serde_json::json;

use super::super::account::json;
use super::super::repos::answered;
use super::super::{check_csrf, Session};
use crate::{tools, Gate};

/// What a person says to the agent.
#[derive(Deserialize)]
struct Say {
    #[serde(rename = "box")]
    boxed: String,
    text: String,
    /// Stop the agent's turn now, then send the message.
    #[serde(default)]
    interrupt: bool,
    ask: Option<String>,
    pick: Option<u32>,
}

/// The requests a message is, in the order they go. Never bare text while a prompt is open: the
/// agent would read it as a menu choice, so the open prompt is turned down by its own entry.
fn plan(body: Option<&[u8]>) -> Result<Vec<SteerReq>, &'static str> {
    let say: Say = body
        .and_then(|b| serde_json::from_slice(b).ok())
        .ok_or("say something to the agent")?;
    let target = BoxName::parse_allow_bare(&say.boxed).map_err(|_| "name a Box")?;
    let text = say.text.trim().to_owned();
    if text.is_empty() {
        return Err("say something to the agent");
    }
    let steer = |text, menu, interrupt, ask| SteerReq {
        target: target.clone().into(),
        text,
        menu,
        interrupt,
        ask,
    };
    Ok(match (say.interrupt, say.pick, say.ask) {
        (true, ..) => vec![
            steer(None, None, true, None),
            steer(Some(text), None, false, None),
        ],
        (false, Some(n), Some(ask)) if n > 0 && !ask.is_empty() => {
            vec![steer(Some(text), Some(n), false, Some(ask))]
        }
        _ => vec![steer(Some(text), None, false, None)],
    })
}

/// Words for a refusal of the message after an interrupt that went through.
fn after_interrupt(reply: &Response) -> String {
    let why = reply.error.as_ref().map_or("", |e| e.message.as_str());
    format!("The agent was interrupted, but your message did not go: {why}")
}

/// Sends the session's person's message through the `steer` that `send_to_box` sends, with an
/// interrupt before it when asked. Answers how the agent took it, or why it did not.
pub(super) fn steer(
    stream: &mut TcpStream,
    gate: &Gate,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let requests = match plan(body) {
        Ok(requests) => requests,
        Err(why) => return json(stream, "400 Bad Request", &json!({"error": why})),
    };
    let last = requests.len() - 1;
    for (i, request) in requests.into_iter().enumerate() {
        let reply = match tools::ask_as(
            &gate.controller,
            &session.person,
            ControllerRequest::Steer(request),
        ) {
            Ok(reply) => reply,
            Err(why) => return json(stream, "502 Bad Gateway", &json!({"error": why})),
        };
        if reply.class != ExitClass::Ok && i > 0 {
            let error = json!({"error": after_interrupt(&reply), "interrupted": true});
            return json(stream, "502 Bad Gateway", &error);
        }
        if reply.class != ExitClass::Ok || i == last {
            return answered(stream, &reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requests(body: &str) -> Vec<SteerReq> {
        plan(Some(body.as_bytes())).unwrap()
    }

    #[test]
    fn a_plain_steer_is_one_text_request() {
        let [only] = requests(r#"{"box": "acme/web-1", "text": " go on "}"#)
            .try_into()
            .unwrap();
        assert_eq!(only.text.as_deref(), Some("go on"));
        assert_eq!((only.menu, only.interrupt, only.ask), (None, false, None));
    }

    #[test]
    fn an_interrupt_stops_the_turn_and_then_sends_the_message() {
        let body =
            r#"{"box": "web-1", "text": "stop that", "interrupt": true, "ask": "ab", "pick": 3}"#;
        let [key, text] = requests(body).try_into().unwrap();
        assert!(key.interrupt && key.text.is_none());
        assert_eq!(text.text.as_deref(), Some("stop that"));
        assert!(!text.interrupt && text.menu.is_none());
    }

    #[test]
    fn a_message_over_an_open_prompt_turns_it_down_by_its_own_entry_in_one_request() {
        let body = r#"{"box": "web-1", "text": "use yarn", "ask": "ab12", "pick": 3}"#;
        let [only] = requests(body).try_into().unwrap();
        assert_eq!((only.menu, only.ask.as_deref()), (Some(3), Some("ab12")));
        assert_eq!(only.text.as_deref(), Some("use yarn"));
    }

    #[test]
    fn a_pick_without_its_prompt_is_not_sent_as_a_menu_choice() {
        let [only] = requests(r#"{"box": "web-1", "text": "hi", "pick": 3}"#)
            .try_into()
            .unwrap();
        assert_eq!(only.menu, None);
    }

    #[test]
    fn a_message_needs_words_and_a_box() {
        for body in [
            r#"{"box": "web-1", "text": "  "}"#,
            r#"{"box": "../x", "text": "hi"}"#,
            r#"{"text": "hi"}"#,
            "not json",
        ] {
            assert!(plan(Some(body.as_bytes())).is_err(), "{body}");
        }
        assert!(plan(None).is_err());
    }
}
