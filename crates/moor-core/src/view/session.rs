//! Typed `value` payloads for the session commands (JSON wire shape).

use crate::name::SessionId;

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

/// Result of `open-host-session`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HostSessionValue {
    /// Id of the new host Session.
    pub session: SessionId,
    /// Directory the agent runs in.
    pub workdir: String,
    /// Session state.
    pub state: String,
}

/// Result of `show-session`: one Session with its children.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionView {
    /// Session id.
    pub session: SessionId,
    /// Box name; `None` for a host Session.
    #[serde(rename = "box")]
    pub box_name: Option<String>,
    /// Position among the Box's Sessions.
    pub ordinal: i64,
    /// Id of the parent Session, if any.
    pub parent: Option<SessionId>,
    /// Directory the agent runs in.
    pub workdir: Option<String>,
    /// Agent that runs the Session.
    pub agent: Option<String>,
    /// Agent's own session id.
    pub agent_session_id: Option<String>,
    /// Session state.
    pub state: String,
    /// Start time, Unix milliseconds.
    pub started_at: i64,
    /// End time, Unix milliseconds; `None` while running.
    pub ended_at: Option<i64>,
    /// Sessions whose parent is this one.
    pub children: Vec<SessionChildView>,
}

/// One child Session in a `SessionView`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionChildView {
    /// Child Session id.
    pub session: SessionId,
    /// Box name; `None` for a host Session.
    #[serde(rename = "box")]
    pub box_name: Option<String>,
    /// Child Session state.
    pub state: String,
}

/// Result of `sessions`: live Sessions ordered by id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionsValue {
    /// The Sessions.
    pub sessions: Vec<SessionListItem>,
}

/// One Session in a `SessionsValue`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionListItem {
    /// Session id.
    pub session: SessionId,
    /// Box name; `None` for a host Session.
    #[serde(rename = "box")]
    pub box_name: Option<String>,
    /// Id of the parent Session, if any.
    pub parent: Option<SessionId>,
    /// Session state.
    pub state: String,
    /// Directory the agent runs in.
    pub workdir: Option<String>,
    /// Position among the Box's Sessions.
    pub ordinal: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        EndSessionValue, HostSessionValue, OpenSessionValue, SessionChildView, SessionListItem,
        SessionView, SessionsValue,
    };
    use crate::name::SessionId;
    use serde_json::json;

    #[test]
    fn session_view_serializes_null_options_and_children() {
        let v = SessionView {
            session: SessionId::new(1),
            box_name: None,
            ordinal: 0,
            parent: None,
            workdir: Some("/w".into()),
            agent: None,
            agent_session_id: Some("a1".into()),
            state: "running".into(),
            started_at: 10,
            ended_at: None,
            children: vec![SessionChildView {
                session: SessionId::new(2),
                box_name: None,
                state: "ended".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&v).expect("serialize"),
            json!({"session": 1, "box": null, "ordinal": 0, "parent": null,
                "workdir": "/w", "agent": null, "agent_session_id": "a1",
                "state": "running", "started_at": 10, "ended_at": null,
                "children": [{"session": 2, "box": null, "state": "ended"}]})
        );
    }

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

    #[test]
    fn host_value_serializes_fields() {
        let v = HostSessionValue {
            session: SessionId::new(9),
            workdir: "/w".into(),
            state: "running".into(),
        };
        assert_eq!(
            serde_json::to_value(&v).expect("serialize"),
            json!({"session": 9, "workdir": "/w", "state": "running"})
        );
    }

    #[test]
    fn sessions_value_round_trips_with_box_key() {
        let v = SessionsValue {
            sessions: vec![
                SessionListItem {
                    session: SessionId::new(1),
                    box_name: None,
                    parent: None,
                    state: "running".into(),
                    workdir: Some("/w".into()),
                    ordinal: 0,
                },
                SessionListItem {
                    session: SessionId::new(2),
                    box_name: Some("demo".into()),
                    parent: Some(SessionId::new(1)),
                    state: "ended".into(),
                    workdir: None,
                    ordinal: 1,
                },
            ],
        };
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(
            json.pointer("/sessions/1/box"),
            Some(&serde_json::Value::from("demo"))
        );
        assert_eq!(
            json.pointer("/sessions/0/box"),
            Some(&serde_json::Value::Null)
        );
        let back: SessionsValue = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, v);
    }

    #[test]
    fn session_view_round_trips_with_child() {
        let v = SessionView {
            session: SessionId::new(1),
            box_name: Some("demo".into()),
            ordinal: 0,
            parent: None,
            workdir: Some("/w".into()),
            agent: Some("claude".into()),
            agent_session_id: None,
            state: "running".into(),
            started_at: 10,
            ended_at: Some(20),
            children: vec![SessionChildView {
                session: SessionId::new(2),
                box_name: None,
                state: "ended".into(),
            }],
        };
        let json = serde_json::to_value(&v).expect("serialize");
        let back: SessionView = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, v);
    }
}
