//! Running setup again on a Box whose setup failed, from the portal.

use std::net::TcpStream;

use moor_core::name::BoxName;
use serde::Deserialize;
use serde_json::json;

use super::super::account::json;
use super::super::{check_csrf, Login, Session};
use crate::{tools, Gate};

/// Which Box to retry.
#[derive(Deserialize)]
struct Retry {
    /// The Box's name, as the list shows it.
    #[serde(rename = "box")]
    boxed: String,
}

/// The Box a body names.
fn target(body: Option<&[u8]>) -> Option<BoxName> {
    let retry: Retry = serde_json::from_slice(body?).ok()?;
    BoxName::parse_allow_bare(&retry.boxed).ok()
}

/// Retries the session's person's Box through the request `retry_box` sends. Answers once setup
/// has restarted, while it carries on. The controller refuses a Box whose setup did not fail,
/// and that refusal is for the person to read.
pub(super) fn retry(
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
    let Some(name) = target(body) else {
        let error = json!({"error": "name a Box"});
        return json(stream, "400 Bad Request", &error);
    };
    let origin: std::sync::Arc<str> = login.public().into();
    match tools::retry_from_portal(&gate.controller, &session.person, name.clone(), origin) {
        Ok(_) => json(stream, "200 OK", &json!({"box": name.as_str()})),
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_must_name_a_box() {
        assert!(target(Some(br#"{"box": "acme/web-1"}"#)).is_some());
        assert!(target(Some(br#"{"box": "web-1"}"#)).is_some());
        for bad in [r#"{"box": "../x"}"#, r#"{"box": ""}"#, "{}", "not json"] {
            assert!(target(Some(bad.as_bytes())).is_none(), "{bad}");
        }
        assert!(target(None).is_none());
    }
}
