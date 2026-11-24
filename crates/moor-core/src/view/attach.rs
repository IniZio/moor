//! Typed `value` payloads for `attach` and `exec` (JSON wire shape).

/// `value` of `attach`, sent after the relay's last bytes frame.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttachView {
    /// Why the relay ended.
    pub ended_by: AttachEnd,
    /// Bytes sent to the caller.
    pub bytes_out: u64,
    /// Bytes taken from the caller.
    pub bytes_in: u64,
}

/// Why an `attach` relay ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachEnd {
    /// The caller shut its side or went away.
    Client,
    /// The agent's terminal session ended.
    Agent,
    /// The link to the Box dropped, or the controller began to restart.
    Link,
}

/// `value` of a place's `exec_in_box` and `exec_wait`: the output since the last answer, and where
/// the command stands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoxRunView {
    /// The command's run, which `exec_wait` names while it is running.
    pub run: crate::request::BoxRunId,
    /// Where the command stands.
    #[serde(flatten)]
    pub state: BoxRunState,
    /// Its stdout since the last answer.
    pub stdout: String,
    /// Its stderr since the last answer.
    pub stderr: String,
    /// True once stdout passed its cap and the rest was cut.
    pub stdout_truncated: bool,
    /// True once stderr passed its cap and the rest was cut.
    pub stderr_truncated: bool,
}

/// Where a place's command stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BoxRunState {
    /// It runs; `exec_wait` with its run reads on.
    Running,
    /// It exited with `exit`.
    Exited {
        /// Its exit code.
        exit: i32,
    },
    /// It ran past its timeout and was killed.
    TimedOut,
    /// It was stopped before it exited, as by a controller restart.
    Stopped,
}

impl From<ExecView> for BoxRunState {
    fn from(view: ExecView) -> Self {
        match view {
            ExecView {
                exit: Some(exit), ..
            } => Self::Exited { exit },
            ExecView {
                timed_out: true, ..
            } => Self::TimedOut,
            ExecView { .. } => Self::Stopped,
        }
    }
}

/// `value` of `exec`, sent after the command's last output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExecView {
    /// The command's exit code; absent when it was killed or the link dropped first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// True when the command ran past the request's timeout and was killed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
}
