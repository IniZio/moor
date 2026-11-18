//! Answering what a Box's agent asks, from the portal.

use std::net::TcpStream;

use moor_core::name::BoxName;
use moor_core::request::{Request as ControllerRequest, SteerReq};
use moor_core::response::ExitClass;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::account::json;
use super::super::repos::answered;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// What an answer says.
#[derive(Deserialize)]
struct Answer {
    /// The Box.
    #[serde(rename = "box")]
    boxed: String,
    /// The prompt answered: the `needs.id` the page showed.
    ask: String,
    /// The menu number picked.
    n: u32,
    /// What the person typed, with the menu entry that opens a text field.
    #[serde(default)]
    text: Option<String>,
}

/// The `steer` an answer is: one menu choice, for the prompt it names, so it lands on no other.
fn request(body: Option<&[u8]>) -> Result<ControllerRequest, &'static str> {
    let said = body
        .and_then(|b| serde_json::from_slice::<Answer>(b).ok())
        .filter(|a| a.n > 0 && !a.ask.is_empty())
        .ok_or("pick one of the answers on the card")?;
    let target = BoxName::parse_allow_bare(&said.boxed).map_err(|_| "name a Box")?;
    Ok(ControllerRequest::Steer(SteerReq {
        target: target.into(),
        text: said
            .text
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty()),
        menu: Some(said.n),
        interrupt: false,
        ask: Some(said.ask),
    }))
}

/// Sends the session's person's answer to their Box's agent, through the `steer` that
/// `send_to_box` uses too. Answers whether the agent took it up.
pub(super) fn answer(
    stream: &mut TcpStream,
    gate: &Gate,
    _login: &Login,
    session: &Session,
    csrf: Option<&str>,
    body: Option<&[u8]>,
) {
    if !csrf.is_some_and(|c| check_csrf(session, c)) {
        let error = json!({"error": "write from the page you are signed in on"});
        return json(stream, "403 Forbidden", &error);
    }
    let steer = match request(body) {
        Ok(steer) => steer,
        Err(why) => return json(stream, "400 Bad Request", &json!({"error": why})),
    };
    match tools::ask_as(&gate.controller, &session.person, steer) {
        Ok(reply) if reply.class == ExitClass::Ok => {
            let value = reply.value.unwrap_or_default();
            let took = value.get("answered_menu").and_then(Value::as_bool);
            json(stream, "200 OK", &json!({"answered": took.unwrap_or(true)}));
        }
        Ok(reply) => answered(stream, &reply),
        Err(why) => json(stream, "502 Bad Gateway", &json!({"error": why})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steer(body: &str) -> Result<SteerReq, &'static str> {
        let ControllerRequest::Steer(s) = request(Some(body.as_bytes()))? else {
            return Err("not a steer");
        };
        Ok(s)
    }

    #[test]
    fn an_answer_is_one_menu_choice_for_the_prompt_it_names() {
        let s = steer(r#"{"box": "acme/web-1", "ask": "ab12", "n": 2}"#).unwrap();
        assert_eq!(s.menu, Some(2));
        assert_eq!(s.ask.as_deref(), Some("ab12"));
        assert_eq!((s.text, s.interrupt), (None, false));
    }

    #[test]
    fn typed_words_ride_with_the_entry_that_opens_the_field() {
        let s = steer(r#"{"box": "web-1", "ask": "ab", "n": 3, "text": " purple "}"#).unwrap();
        assert_eq!((s.menu, s.text.as_deref()), (Some(3), Some("purple")));
        let blank = steer(r#"{"box": "web-1", "ask": "ab", "n": 3, "text": "  "}"#).unwrap();
        assert_eq!(blank.text, None);
    }

    #[test]
    fn an_answer_missing_its_prompt_or_its_choice_is_refused() {
        for body in [
            r#"{"box": "web-1", "ask": "", "n": 1}"#,
            r#"{"box": "web-1", "ask": "ab", "n": 0}"#,
            r#"{"box": "web-1", "n": 1}"#,
            r#"{"box": "../x", "ask": "ab", "n": 1}"#,
            "not json",
        ] {
            assert!(steer(body).is_err(), "{body}");
        }
        assert!(request(None).is_err());
    }
}
