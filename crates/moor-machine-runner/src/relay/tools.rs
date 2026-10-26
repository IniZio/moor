//! How a relay's tools are listed to a Box: renamed under the relay and with forced arguments
//! hidden.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rmcp::model::Tool;
use rmcp::ErrorData;
use serde_json::{Map, Value};

use super::Relay;
use crate::hooks::Facts;

/// How a listed tool reaches its relay.
#[derive(Clone)]
pub(super) struct Route {
    /// The tool's name on the relay.
    pub(super) name: String,
    /// The forced arguments the tool takes: each with the fact it is set from and its type.
    pub(super) forced: Vec<(String, String, Kind)>,
    /// Whether the operator hid the tool: it is not listed and a call to it is refused.
    pub(super) hidden: bool,
}

impl Route {
    /// `arguments` as the agent sent them, with the forced ones set from `facts`; an error when
    /// the tool is hidden, a fact is missing, or a fact is not of its argument's type.
    pub(super) fn arguments(
        &self,
        listed: &str,
        facts: &Facts,
        sent: Option<Map<String, Value>>,
    ) -> Result<Map<String, Value>, ErrorData> {
        if self.hidden {
            return Err(ErrorData::invalid_request(
                format!("{listed} is hidden on this machine"),
                None,
            ));
        }
        let mut arguments = sent.unwrap_or_default();
        for (arg, fact, kind) in &self.forced {
            let value = facts.get(fact).ok_or_else(|| {
                ErrorData::invalid_request(
                    format!(
                        "this Box has no fact {fact}, which {listed} needs; the machine's \
                         prepare command sets it"
                    ),
                    None,
                )
            })?;
            let value = kind.value(value).map_err(|wanted| {
                ErrorData::invalid_request(
                    format!(
                        "fact {fact} is {value:?}, not {wanted}, which {listed} needs for {arg}"
                    ),
                    None,
                )
            })?;
            arguments.insert(arg.clone(), value);
        }
        Ok(arguments)
    }
}

/// What a scalar argument's schema asks for.
#[derive(Clone, Copy)]
enum Scalar {
    String,
    Integer,
    Number,
}

/// The JSON type a forced argument takes in the tool's schema.
#[derive(Clone, Copy)]
pub(super) struct Kind {
    scalar: Scalar,
    /// Whether the argument is an array, of which the fact is the only item.
    array: bool,
}

impl Kind {
    /// The type `schema` (one property's schema) asks for; an error for any other type.
    fn of(schema: &Value) -> Result<Self, String> {
        let scalar = |schema: &Value| match schema.get("type").and_then(Value::as_str) {
            None | Some("string") => Ok(Scalar::String),
            Some("integer") => Ok(Scalar::Integer),
            Some("number") => Ok(Scalar::Number),
            Some(other) => Err(other.to_owned()),
        };
        if schema.get("type").and_then(Value::as_str) == Some("array") {
            let items = schema.get("items").unwrap_or(&Value::Null);
            return Ok(Self {
                scalar: scalar(items)?,
                array: true,
            });
        }
        Ok(Self {
            scalar: scalar(schema)?,
            array: false,
        })
    }

    /// `fact` as the JSON value the argument takes; an error when it is not of the type.
    pub(super) fn value(self, fact: &str) -> Result<Value, String> {
        let item = match self.scalar {
            Scalar::String => Value::String(fact.to_owned()),
            Scalar::Integer => fact
                .trim()
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| "a whole number".to_owned())?,
            Scalar::Number => fact
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| "a number".to_owned())?,
        };
        Ok(if self.array {
            Value::Array(vec![item])
        } else {
            item
        })
    }
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
        if relay.hidden.contains(&original) {
            routes.insert(
                mcp_name(&format!("{}_{original}", relay.name)),
                Route {
                    name: original,
                    forced: Vec::new(),
                    hidden: true,
                },
            );
            continue;
        }
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
                .and_then(|p| p.remove(arg));
            if let Some(property) = had {
                let kind = Kind::of(&property).map_err(|other| {
                    format!(
                        "its tool {original} takes {arg} as {other}, which --force-arg cannot set"
                    )
                })?;
                if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
                    required.retain(|r| r != arg.as_str());
                }
                forced.push((arg.clone(), fact.clone(), kind));
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
                hidden: false,
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
