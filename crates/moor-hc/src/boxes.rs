//! The person's open Boxes, and whether one is up: ready, with the agent's hc running in it.
//! A Box gets its machine once it is up and keeps it until it closes; only an up one is dialed.

use std::collections::BTreeMap;

use moor_client::Client;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::request::Request;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::Error;

/// The fields of a `show` Box view moor-hc reads.
#[derive(Debug, Deserialize)]
struct Row {
    /// The Box's name.
    name: String,
    /// The Box's lifecycle.
    lifecycle: BoxLifecycle,
    /// The agent, when the controller read it.
    agent: Option<AgentRow>,
}

/// The fields of a `show` agent view moor-hc reads.
#[derive(Debug, Deserialize)]
struct AgentRow {
    /// Whether the agent, and the hc holding its terminal, was launched.
    #[serde(default)]
    launched: bool,
}

impl Row {
    /// Not closed: still shown, and still a machine in hc.
    const fn open(&self) -> bool {
        !matches!(
            self.lifecycle,
            BoxLifecycle::Closed | BoxLifecycle::Rejected
        )
    }

    /// Ready, with the hc that holds the agent's terminal running.
    fn up(&self) -> bool {
        self.lifecycle == BoxLifecycle::Ready && self.agent.as_ref().is_some_and(|a| a.launched)
    }
}

/// The `value` of `show`: every open Box, or the one asked for.
#[derive(Debug, Deserialize)]
struct ShowValue {
    /// Every open Box, when no Box was named.
    #[serde(default)]
    boxes: Vec<Row>,
    /// The Box named.
    #[serde(rename = "box")]
    one: Option<Row>,
}

/// The `show` value the controller answers `wire` with.
fn show(client: &Client, wire: Value) -> Result<ShowValue, Error> {
    let reply = client.request(serde_json::from_value::<Request>(wire)?, |_| {})?;
    if let Some(e) = reply.error {
        return Err(Error::Refused(e.message));
    }
    Ok(serde_json::from_value(reply.value.unwrap_or(Value::Null))?)
}

/// The person's open Boxes, each mapped to whether it is up.
pub fn open_boxes(client: &Client) -> Result<BTreeMap<String, bool>, Error> {
    let shown = show(client, json!({"cmd": "show"}))?;
    Ok(shown
        .boxes
        .into_iter()
        .filter(Row::open)
        .map(|r| {
            let up = r.up();
            (r.name, up)
        })
        .collect())
}

/// Whether Box `name` is up; an unknown Box is not.
pub fn is_up(client: &Client, name: &str) -> Result<bool, Error> {
    let shown = show(client, json!({"cmd": "show", "box": name}))?;
    Ok(shown.one.is_some_and(|r| r.up()))
}
