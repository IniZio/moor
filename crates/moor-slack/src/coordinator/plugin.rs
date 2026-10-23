//! The plugins an agent's init event lists.

use serde_json::Value;

/// One plugin an init event lists.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Plugin {
    /// Its name.
    pub(crate) name: String,
    /// Whether its source is `@builtin`, a plugin that ships inside the agent's binary.
    pub(crate) builtin: bool,
}

impl Plugin {
    /// The plugins `event` lists. One is built in only when its `source` ends in `@builtin`;
    /// one with no source is not.
    pub(crate) fn listed(event: &Value) -> Vec<Self> {
        let items = event.get("plugins").and_then(Value::as_array);
        items
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let name = item.get("name").and_then(Value::as_str)?.to_owned();
                let source = item.get("source").and_then(Value::as_str);
                let builtin = source.is_some_and(|s| s.ends_with("@builtin"));
                Some(Self { name, builtin })
            })
            .collect()
    }
}
