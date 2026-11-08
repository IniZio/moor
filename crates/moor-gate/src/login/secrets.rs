//! The secrets a person's Boxes need and whether each is stored, for the account page.

use moor_core::request::{AsReq, Request as ControllerRequest, StartNeedsReq};
use moor_core::view::user::{SecretNamesValue, StartNeedsValue};

use super::Session;
use crate::{tools, Gate};

/// The secrets every Box on GitHub needs.
const BASE: [&str; 2] = ["claude", "github"];

/// One secret as the page lists it.
#[derive(serde::Serialize)]
pub(super) struct Row {
    /// The secret's name.
    pub(super) name: String,
    /// Whether the person has stored it.
    #[serde(rename = "linked")]
    pub(super) set: bool,
}

/// The secrets the person's Boxes need and whether each is set, asked as the session's person.
pub(super) fn rows(gate: &Gate, session: &Session) -> Result<Vec<Row>, String> {
    let ask = |request| tools::act_as(&gate.controller, &session.person, request);
    let unreadable = |e: serde_json::Error| format!("the controller's answer is unreadable: {e}");
    let needs = ask(ControllerRequest::StartNeeds(StartNeedsReq {
        identity: None,
        repo: None,
    }))?;
    let needs: StartNeedsValue = serde_json::from_value(needs).map_err(unreadable)?;
    let have = ask(ControllerRequest::SecretNames(AsReq { identity: None }))?;
    let have: SecretNamesValue = serde_json::from_value(have).map_err(unreadable)?;
    let mut names: Vec<String> = BASE.iter().map(|n| (*n).to_owned()).collect();
    for name in needs.needs {
        if !names.iter().any(|n| n == name.as_str()) {
            names.push(name.as_str().to_owned());
        }
    }
    Ok(names
        .into_iter()
        .map(|name| Row {
            set: have.names.iter().any(|h| h.as_str() == name),
            name,
        })
        .collect())
}
