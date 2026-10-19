//! Typed `value` payload for `end-session` (JSON wire shape).

/// Result of `end-session`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EndSessionValue {
    /// Task name.
    pub task: String,
    /// Id of the Session that ended.
    pub session: i64,
    /// Position of that Session among the task's Sessions.
    pub ordinal: i64,
    /// Whether the Box stayed open.
    pub box_kept: bool,
}

/// Result of `open-session`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OpenSessionValue {
    /// Task name.
    pub task: String,
    /// Id of the new Session.
    pub session: i64,
    /// Position of the new Session among the task's Sessions.
    pub ordinal: i64,
    /// Pane the agent runs in, when known.
    pub pane: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{EndSessionValue, OpenSessionValue};
    use serde_json::json;

    #[test]
    fn value_serializes_four_fields() {
        let v = EndSessionValue {
            task: "x".into(),
            session: 7,
            ordinal: 2,
            box_kept: true,
        };
        assert_eq!(
            serde_json::to_value(&v).expect("serialize"),
            json!({"task": "x", "session": 7, "ordinal": 2, "box_kept": true})
        );
    }

    #[test]
    fn open_value_serializes_fields() {
        let v = OpenSessionValue {
            task: "x".into(),
            session: 8,
            ordinal: 3,
            pane: Some("p1".into()),
        };
        assert_eq!(
            serde_json::to_value(&v).expect("serialize"),
            json!({"task": "x", "session": 8, "ordinal": 3, "pane": "p1"})
        );
    }
}
