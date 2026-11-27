//! Arguments of `box-tool`, which the gate sends for the agent in an agent place: a command in
//! one of its user's work Boxes, a new Box on its task's repos, or the close of a work Box.

use serde::{Deserialize, Serialize};

use crate::name::{BoxId, BoxName, ForgeRepo};

/// Seconds a Box-tool command may run when the call names none, as a person's `exec_in_box`.
pub const BOX_EXEC_DEFAULT_SECS: u32 = 30;

/// Most seconds a Box-tool command may run: as long as a gate check.
pub const BOX_EXEC_MAX_SECS: u32 = 1800;

/// The id of a command a place started with `exec_in_box`, which `exec_wait` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoxRunId(pub u32);

/// Arguments of `box-tool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxToolReq {
    /// The Box whose agent called the tool. The gate takes it from `X-Moor-Box`, which only the
    /// Proxy sets, and never from the agent's arguments.
    pub from: BoxId,
    /// What the agent asked for.
    pub call: BoxCall,
}

/// One Box-tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoxCall {
    /// Run a command in one of the user's work Boxes.
    Exec {
        /// The work Box.
        target: BoxName,
        /// The command, as shell text.
        command: String,
        /// When the command is killed.
        timeout: BoxExecSecs,
    },
    /// Start a Box for the user on the task's repos, tied to the task and running no agent.
    Start {
        /// The new Box's name.
        name: BoxName,
        /// The repo the Box clones.
        repo: ForgeRepo,
        /// Repos it clones beside `repo`, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<ForgeRepo>,
        /// Skip the repo's setup lines on every machine the Box gets.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_setup: bool,
        /// The revision of `repo` to branch from; its default branch when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// Close one of the user's open work Boxes. It has no `force`: a Box that holds work not
    /// pushed stays open, as a person's `moor close` leaves it.
    Close {
        /// The work Box.
        target: BoxName,
    },
    /// Wait on a command an `exec` started that was still running when it answered.
    Wait {
        /// The command's run.
        run: BoxRunId,
    },
}

/// Seconds a Box-tool command may run: 1 to [`BOX_EXEC_MAX_SECS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BoxExecSecs(u32);

impl BoxExecSecs {
    /// The seconds `asked` names, or [`BOX_EXEC_DEFAULT_SECS`] when it names none.
    ///
    /// # Errors
    /// A sentence for a number outside 1 to [`BOX_EXEC_MAX_SECS`].
    pub fn new(asked: Option<u32>) -> Result<Self, String> {
        match asked.unwrap_or(BOX_EXEC_DEFAULT_SECS) {
            secs @ 1..=BOX_EXEC_MAX_SECS => Ok(Self(secs)),
            secs => Err(format!(
                "timeout_seconds must be 1 to {BOX_EXEC_MAX_SECS}, not {secs}"
            )),
        }
    }

    /// The seconds.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for BoxExecSecs {
    type Error = String;

    fn try_from(secs: u32) -> Result<Self, String> {
        Self::new(Some(secs))
    }
}

impl From<BoxExecSecs> for u32 {
    fn from(secs: BoxExecSecs) -> Self {
        secs.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_defaults_to_thirty_seconds_and_stops_at_thirty_minutes() {
        assert_eq!(BoxExecSecs::new(None).map(BoxExecSecs::get), Ok(30));
        assert_eq!(BoxExecSecs::new(Some(1800)).map(BoxExecSecs::get), Ok(1800));
        assert!(BoxExecSecs::new(Some(0)).is_err());
        assert!(BoxExecSecs::new(Some(1801)).is_err());
        let wire =
            r#"{"from":3,"call":{"tool":"exec","target":"w","command":"ls","timeout":1801}}"#;
        assert!(serde_json::from_str::<BoxToolReq>(wire).is_err());
    }

    #[test]
    fn a_wait_names_only_its_run() {
        let wire = r#"{"from":3,"call":{"tool":"wait","run":7}}"#;
        let req: BoxToolReq = serde_json::from_str(wire).unwrap();
        assert_eq!(req.call, BoxCall::Wait { run: BoxRunId(7) });
        let more = r#"{"from":3,"call":{"tool":"wait","run":7,"box":"w"}}"#;
        assert!(serde_json::from_str::<BoxToolReq>(more).is_err());
    }

    #[test]
    fn a_close_names_a_box_and_offers_no_force() {
        let wire = r#"{"from":3,"call":{"tool":"close","target":"w"}}"#;
        let req: BoxToolReq = serde_json::from_str(wire).unwrap();
        assert!(matches!(req.call, BoxCall::Close { .. }));
        let forced = r#"{"from":3,"call":{"tool":"close","target":"w","force":true}}"#;
        assert!(serde_json::from_str::<BoxToolReq>(forced).is_err());
    }
}
