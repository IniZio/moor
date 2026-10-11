pub mod old;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::world::World;

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
    pub name: String,
    pub state: TaskState,
    pub boxed: Option<BoxView>,
    pub worktree: Option<String>,
    pub workspace_id: Option<String>,
    raw: Value,
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

impl TaskView {
    pub fn from_json(v: &Value) -> TaskView {
        let boxed = v.get("box").filter(|b| b.is_object()).map(|b| BoxView {
            state: BoxState::parse(b.get("state").and_then(Value::as_str).unwrap_or("")),
            handle: opt_str(b, "handle"),
        });
        TaskView {
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

pub trait Moor {
    fn name(&self) -> &'static str;
    fn caps(&self) -> &'static [&'static str] {
        &[]
    }
    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView>;
    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>>;
    fn close(&self, w: &World, cwd: &Path, name: &str, force: bool) -> Outcome<CloseView>;
    fn events(&self, w: &World, name: &str) -> Vec<Event>;
    fn run(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        argv: &[&str],
        opts: &RunOpts,
    ) -> Outcome<RunResult>;
    fn cp(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        box_path: &str,
        host_path: &Path,
    ) -> Outcome<()>;
    fn gate_check(
        &self,
        w: &World,
        cwd: &Path,
        sha: &str,
        name: &str,
        command: Option<&str>,
    ) -> Outcome<GateVerdict>;
    fn runs(&self, w: &World, name: &str) -> Vec<RunRow>;
    fn seed_run(&self, w: &World, row: &RunRow);
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

pub fn from_env() -> Result<Box<dyn Moor>, String> {
    let bin = std::env::var_os("MOOR_BIN").ok_or("MOOR_BIN is not set")?;
    let bin = PathBuf::from(bin);
    if !bin.exists() {
        return Err(format!("MOOR_BIN does not exist: {}", bin.display()));
    }
    match std::env::var("MOOR_CLI").as_deref() {
        Err(_) | Ok("old") => Ok(Box::new(old::OldCli { bin })),
        Ok("new") => Err("new adapter not implemented".to_string()),
        Ok(other) => Err(format!("unknown MOOR_CLI: {other}")),
    }
}

pub fn exit_class(code: i32) -> ExitClass {
    match code {
        0 => ExitClass::Ok,
        2 => ExitClass::Usage,
        _ => ExitClass::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_state_parse() {
        let cases = [
            ("checking", TaskState::Checking),
            ("rejected", TaskState::Rejected),
            ("waiting_for_box", TaskState::WaitingForBox),
            ("setting_up", TaskState::SettingUp),
            ("working", TaskState::Working),
            ("setup_failed", TaskState::SetupFailed),
            ("idle", TaskState::Idle),
            ("asleep", TaskState::Asleep),
            ("closed", TaskState::Closed),
            ("weird", TaskState::Other("weird".into())),
        ];
        for (s, want) in cases {
            assert_eq!(TaskState::parse(s), want);
        }
    }

    #[test]
    fn box_state_parse() {
        let cases = [
            ("starting", BoxState::Starting),
            ("ready", BoxState::Ready),
            ("failed", BoxState::Failed),
            ("gone", BoxState::Gone),
            ("stopped", BoxState::Stopped),
            ("deleted", BoxState::Deleted),
            ("release_failed", BoxState::ReleaseFailed),
            ("asleep", BoxState::Asleep),
            ("weird", BoxState::Other("weird".into())),
        ];
        for (s, want) in cases {
            assert_eq!(BoxState::parse(s), want);
        }
    }

    #[test]
    fn event_kind_from_old() {
        let cases = [
            ("started", EventKind::Started),
            ("rejected", EventKind::Rejected),
            ("waiting_for_box", EventKind::WaitingForBox),
            ("setting_up", EventKind::SettingUp),
            ("working", EventKind::Working),
            ("setup_failed", EventKind::SetupFailed),
            ("idled", EventKind::Idled),
            ("busy", EventKind::Busy),
            ("asleep", EventKind::Asleep),
            ("woke", EventKind::Woke),
            ("closed", EventKind::Closed),
            ("box_ready", EventKind::BoxReady),
            ("box_failed", EventKind::BoxFailed),
            ("box_gone", EventKind::BoxGone),
            ("box_deleted", EventKind::BoxDeleted),
            ("box_asleep", EventKind::BoxAsleep),
            ("session_started", EventKind::SessionStarted),
            ("ran", EventKind::Ran),
            ("copied", EventKind::Copied),
            ("credential_failed", EventKind::CredentialFailed),
            ("weird", EventKind::Other("weird".into())),
        ];
        for (s, want) in cases {
            assert_eq!(EventKind::from_old(s), want);
        }
    }

    #[test]
    fn exit_class_is_err() {
        assert!(!ExitClass::Ok.is_err());
        assert!(ExitClass::Usage.is_err());
        assert!(ExitClass::Refused.is_err());
        assert!(ExitClass::Failed.is_err());
    }

    #[test]
    fn exit_classes() {
        assert_eq!(exit_class(0), ExitClass::Ok);
        assert_eq!(exit_class(2), ExitClass::Usage);
        assert_eq!(exit_class(1), ExitClass::Failed);
    }

    #[test]
    fn task_view_from_json() {
        let v = json!({
            "id": 1, "name": "t1", "state": "working", "worktree": "/w/t1",
            "workspace": "moor:t1", "workspace_id": "ws-1",
            "box": {"driver": "crabbox", "handle": "local/moor-t1-abc", "state": "ready"}
        });
        let t = TaskView::from_json(&v);
        assert_eq!(t.name, "t1");
        assert_eq!(t.state, TaskState::Working);
        assert_eq!(t.worktree.as_deref(), Some("/w/t1"));
        assert_eq!(t.workspace_id.as_deref(), Some("ws-1"));
        let b = t.boxed.unwrap();
        assert_eq!(b.state, BoxState::Ready);
        assert_eq!(b.handle.as_deref(), Some("local/moor-t1-abc"));

        let t = TaskView::from_json(&json!({"name": "n", "state": "closed", "box": null}));
        assert!(t.boxed.is_none());
    }
}
