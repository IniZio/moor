//! How a relay's tools are listed to a Box: renamed under the relay and with forced arguments
//! hidden.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rmcp::model::Tool;
use serde_json::Value;

use super::Relay;

/// How a listed tool reaches its relay.
#[derive(Clone)]
pub(super) struct Route {
    /// The tool's name on the relay.
    pub(super) name: String,
    /// The forced arguments the tool takes, with the fact each is set from.
    pub(super) forced: Vec<(String, String)>,
}

/// The relay's tools renamed for the Box with their forced arguments hidden, and how each
/// reaches the relay; an error when two names collide.
pub(super) fn expose(
    relay: &Relay,
    listed: Vec<Tool>,
    taken: &[String],
) -> Result<(Vec<Tool>, HashMap<String, Route>), String> {
    let mut names: HashSet<String> = taken.iter().cloned().collect();
    let mut tools = Vec::new();
    let mut routes = HashMap::new();
    for mut tool in listed {
        let original = tool.name.to_string();
        let name = mcp_name(&format!("{}_{original}", relay.name));
        if !names.insert(name.clone()) {
            return Err(format!(
                "its tool {original} would be listed as {name}, which is taken"
            ));
        }
        let mut schema = (*tool.input_schema).clone();
        let mut forced = Vec::new();
        for (arg, fact) in &relay.forced {
            let had = schema
                .get_mut("properties")
                .and_then(Value::as_object_mut)
                .and_then(|p| p.remove(arg))
                .is_some();
            if had {
                if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
                    required.retain(|r| r != arg.as_str());
                }
                forced.push((arg.clone(), fact.clone()));
            }
        }
        tool.input_schema = Arc::new(schema);
        tool.name = name.clone().into();
        tools.push(tool);
        routes.insert(
            name,
            Route {
                name: original,
                forced,
            },
        );
    }
    Ok((tools, routes))
}

/// `name` with every character outside `[A-Za-z0-9_]` as `_`.
pub(super) fn mcp_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
