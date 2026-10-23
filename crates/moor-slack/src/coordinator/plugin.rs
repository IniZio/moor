//! The plugins an agent's init event lists.

use serde_json::Value;

/// One plugin an init event lists.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Plugin {
    /// Its name.
    pub(crate) name: String,
    /// Whether its source is `@builtin` and its path is `builtin`: a plugin inside the agent's binary.
    pub(crate) builtin: bool,
}

impl Plugin {
    /// The plugins `event` lists. One is built in only when its `source` ends in `@builtin` and its
    /// `path` is `builtin`; a marketplace may be named `builtin`.
    pub(crate) fn listed(event: &Value) -> Vec<Self> {
        let items = event.get("plugins").and_then(Value::as_array);
        items
            .into_iter()
            .flatten()
            .map(|item| {
                let name = item.get("name").and_then(Value::as_str);
                let name = name.unwrap_or("(unnamed)").to_owned();
                let source = item.get("source").and_then(Value::as_str);
                let path = item.get("path").and_then(Value::as_str);
                let builtin =
                    source.is_some_and(|s| s.ends_with("@builtin")) && path == Some("builtin");
                Self { name, builtin }
            })
            .collect()
    }
}
