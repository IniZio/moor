//! Typed `value` payloads for the binding commands (JSON wire shape).

use crate::name::SessionId;

/// A conversation key bound to a Session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BindingValue {
    /// Front the key belongs to.
    pub front: String,
    /// Conversation key within that front.
    pub key: String,
    /// Id of the bound Session.
    pub session: SessionId,
    /// Box name; None for a host Session.
    pub task: Option<String>,
    /// Who owns the binding.
    pub owner: String,
}

/// Result of `bind-session`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoundValue {
    /// True when this call created the binding; false when the key was already bound.
    pub created: bool,
    /// The binding now in force.
    pub binding: BindingValue,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_binding_serializes_null_task() {
        let value = BoundValue {
            created: true,
            binding: BindingValue {
                front: "f".into(),
                key: "k".into(),
                session: SessionId::new(2),
                task: None,
                owner: "o".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&value).expect("serialize"),
            json!({"created": true, "binding": {
                "front": "f", "key": "k", "session": 2, "task": null, "owner": "o"}})
        );
    }
}
