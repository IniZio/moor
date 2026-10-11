use std::path::PathBuf;

use serde_json::Value;

/// Exit classification. The old adapter never yields `Refused`; only the new
/// adapter does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitClass {
    Ok,
    Usage,
    Refused,
    Failed,
}

impl ExitClass {
    pub fn is_err(self) -> bool {
        matches!(self, Self::Usage | Self::Refused | Self::Failed)
    }
}

#[derive(Debug, Clone)]
pub struct Outcome<T> {
    pub exit: i32,
    pub class: ExitClass,
    pub stdout: String,
    pub stderr: String,
    pub warnings: Vec<String>,
    pub value: Option<T>,
}

impl<T> Outcome<T> {
    pub fn is_err(&self) -> bool {
        self.class.is_err()
    }

    pub fn ok(&self) -> &T {
        match (&self.class, &self.value) {
            (ExitClass::Ok, Some(v)) => v,
            _ => panic!(
                "expected ok outcome: exit={} class={:?}\nstdout: {}\nstderr: {}",
                self.exit, self.class, self.stdout, self.stderr
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Checking,
    Rejected,
    WaitingForBox,
    SettingUp,
    Working,
    SetupFailed,
    Idle,
    Asleep,
    Closed,
    Other(String),
}

impl TaskState {
    pub fn parse(s: &str) -> Self {
        match s {
            "checking" => Self::Checking,
            "rejected" => Self::Rejected,
            "waiting_for_box" => Self::WaitingForBox,
            "setting_up" => Self::SettingUp,
            "working" => Self::Working,
            "setup_failed" => Self::SetupFailed,
            "idle" => Self::Idle,
            "asleep" => Self::Asleep,
            "closed" => Self::Closed,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxState {
    Starting,
    Ready,
    Failed,
    Gone,
    Stopped,
    Deleted,
    ReleaseFailed,
    Asleep,
    Other(String),
}

impl BoxState {
    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => Self::Starting,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            "gone" => Self::Gone,
            "stopped" => Self::Stopped,
            "deleted" => Self::Deleted,
            "release_failed" => Self::ReleaseFailed,
            "asleep" => Self::Asleep,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoxView {
    pub state: BoxState,
    pub handle: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskView {
    pub id: Option<i64>,
    pub base: Option<String>,
    pub name: String,
    pub state: TaskState,
    pub boxed: Option<BoxView>,
    pub worktree: Option<String>,
    pub workspace_id: Option<String>,
    pub(super) raw: Value,
}

pub(super) fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

impl TaskView {
    pub fn from_json(v: &Value) -> TaskView {
        let boxed = v.get("box").filter(|b| b.is_object()).map(|b| BoxView {
            state: BoxState::parse(b.get("state").and_then(Value::as_str).unwrap_or("")),
            handle: opt_str(b, "handle"),
        });
        TaskView {
            id: v.get("id").and_then(Value::as_i64),
            base: opt_str(v, "base"),
            name: opt_str(v, "name").unwrap_or_default(),
            state: TaskState::parse(v.get("state").and_then(Value::as_str).unwrap_or("")),
            boxed,
            worktree: opt_str(v, "worktree"),
            workspace_id: opt_str(v, "workspace_id"),
            raw: v.clone(),
        }
    }

    /// Debug-only access to the adapter's raw JSON; never use in `both` tests.
    pub fn adapter_debug_raw(&self) -> &Value {
        &self.raw
    }
}

#[derive(Debug, Clone)]
pub struct CloseView {
    pub task: TaskView,
    pub box_deleted: bool,
    pub workspace_closed: bool,
    pub sessions_stopped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Started,
    Rejected,
    WaitingForBox,
    SettingUp,
    Working,
    SetupFailed,
    Idled,
    Busy,
    Asleep,
    Woke,
    Closed,
    BoxReady,
    BoxFailed,
    BoxGone,
    BoxDeleted,
    BoxAsleep,
    SessionStarted,
    Ran,
    Copied,
    CredentialFailed,
    Other(String),
}

impl EventKind {
    pub fn from_old(s: &str) -> Self {
        match s {
            "started" => Self::Started,
            "rejected" => Self::Rejected,
            "waiting_for_box" => Self::WaitingForBox,
            "setting_up" => Self::SettingUp,
            "working" => Self::Working,
            "setup_failed" => Self::SetupFailed,
            "idled" => Self::Idled,
            "busy" => Self::Busy,
            "asleep" => Self::Asleep,
            "woke" => Self::Woke,
            "closed" => Self::Closed,
            "box_ready" => Self::BoxReady,
            "box_failed" => Self::BoxFailed,
            "box_gone" => Self::BoxGone,
            "box_deleted" => Self::BoxDeleted,
            "box_asleep" => Self::BoxAsleep,
            "session_started" => Self::SessionStarted,
            "ran" => Self::Ran,
            "copied" => Self::Copied,
            "credential_failed" => Self::CredentialFailed,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    pub payload: Value,
}

#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    pub issue: Option<String>,
    pub goal: Option<String>,
    pub worktree: Option<PathBuf>,
    pub location: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentView {
    pub state: String,
    pub agent_state: Option<String>,
    pub source: Option<String>,
    pub exit_code: Option<i64>,
    pub waiting_for: Option<String>,
}

impl AgentView {
    pub fn from_json(v: &Value) -> Option<AgentView> {
        v.is_object().then(|| AgentView {
            state: opt_str(v, "state").unwrap_or_default(),
            agent_state: opt_str(v, "agent_state"),
            source: opt_str(v, "agent_state_source"),
            exit_code: v.get("exit_code").and_then(Value::as_i64),
            waiting_for: opt_str(v, "waiting_for"),
        })
    }
}

impl TaskView {
    pub fn agent(&self) -> Option<AgentView> {
        self.raw.get("agent").and_then(AgentView::from_json)
    }

    pub fn sleep_reason(&self) -> Option<String> {
        opt_str(&self.raw, "sleep")
    }
}

#[derive(Debug, Clone)]
pub struct WaitView {
    pub timed_out: bool,
    pub run: Option<Value>,
    pub agent: Option<AgentView>,
}

#[derive(Debug, Clone)]
pub struct OpenView {
    pub task: TaskView,
    pub woke: bool,
}

#[derive(Debug, Clone)]
pub struct PaneView {
    pub task: TaskView,
    pub agent: Option<AgentView>,
}

/// Options for `Moor::run`. `interrupt` sends SIGINT to moor once the Driver
/// has recorded its run call, so the test needs no wall-clock sleeps.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    pub no_sync: bool,
    pub quiet: bool,
    pub record: Option<String>,
    pub interrupt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit: i32,
    pub interrupted: bool,
    pub sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateVerdict {
    pub ok: bool,
    pub sha: Option<String>,
    pub exit: Option<i32>,
    pub task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub task: String,
    pub sha: String,
    pub name: String,
    pub command: String,
    pub exit: i32,
    pub interrupted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub box_pane: bool,
    pub live: bool,
}

#[derive(Debug, Clone)]
pub struct RemoveView {
    pub task: TaskView,
    pub worktree_removed: bool,
    pub branch_deleted: bool,
    pub discarded: bool,
}
