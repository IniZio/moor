pub mod github_ops;
pub mod old;
pub mod reports;

pub use reports::{ReportReq, ReportRow};
pub mod old_github;
mod old_proxy;
pub mod proxy;

pub use proxy::{ProxyCheck, ProxyEndpoint};

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
    pub id: Option<i64>,
    pub base: Option<String>,
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

pub trait Moor {
    fn name(&self) -> &'static str;
    fn caps(&self) -> &'static [&'static str] {
        &[]
    }
    fn start(&self, w: &World, cwd: &Path, name: &str, opts: &StartOpts) -> Outcome<TaskView>;
    fn list(&self, w: &World, cwd: &Path) -> Outcome<Vec<TaskView>>;
    fn close(&self, w: &World, cwd: &Path, name: &str, force: bool) -> Outcome<CloseView>;
    fn events(&self, w: &World, name: &str) -> Vec<Event>;
    fn env(&self, w: &World, cwd: &Path, opts: &EnvOpts) -> Outcome<EnvReport>;
    fn remove(&self, w: &World, cwd: &Path, name: &str, discard: bool) -> Outcome<RemoveView>;
    fn sessions(&self, w: &World, name: &str) -> Vec<Session>;
    fn age(&self, w: &World, name: &str, secs: i64);
    fn start_detached(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        opts: &StartOpts,
    ) -> std::process::Child;
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
    fn wait(
        &self,
        w: &World,
        cwd: &Path,
        name: &str,
        timeout_secs: Option<u64>,
    ) -> Outcome<WaitView>;
    fn steer(&self, w: &World, cwd: &Path, name: &str, message: &str) -> Outcome<()>;
    fn open(&self, w: &World, cwd: &Path, name: &str) -> Outcome<OpenView>;
    fn shell(&self, w: &World, cwd: &Path, name: &str) -> Outcome<()>;
    fn pane(&self, w: &World, cwd: &Path, name: &str) -> Outcome<PaneView>;
    fn run_bg(&self, w: &World, cwd: &Path, name: &str, command: &str) -> std::process::Child;
    fn sleep_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView>;
    fn wake_cmd(&self, w: &World, cwd: &Path, name: &str) -> Outcome<TaskView>;
    fn report(&self, w: &World, cwd: &Path, name: &str, req: &ReportReq) -> Outcome<ReportRow>;
    fn reports(&self, w: &World, cwd: &Path, name: &str) -> Outcome<Vec<ReportRow>>;
    fn version(&self, w: &World) -> Outcome<String>;
    fn github_setup(
        &self,
        w: &World,
        cwd: &Path,
        opts: &github_ops::GithubSetupOpts,
    ) -> Outcome<github_ops::GithubStore>;
    fn github_login(&self, w: &World, cwd: &Path) -> Outcome<github_ops::GithubStore>;
    fn github_raw(&self, w: &World, cwd: &Path, args: &[&str]) -> Outcome<github_ops::GithubStore>;
    fn proxy_check(&self, w: &World, cwd: &Path, name: &str) -> Outcome<ProxyCheck>;
    fn proxy_endpoint(&self, w: &World, name: &str) -> Option<ProxyEndpoint>;
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

#[derive(Debug, Clone, Default)]
pub struct EnvOpts {
    pub agent_location: Option<String>,
    pub text: bool,
    pub set_env: Vec<(String, String)>,
    pub unset_env: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EnvItem {
    pub kind: String,
    pub name: String,
    pub source: String,
    pub detail: Option<String>,
    pub resolves: Option<bool>,
    pub error: Option<String>,
    pub delivery: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnvReport {
    pub repo: String,
    pub location: String,
    pub model: Option<String>,
    pub identity: String,
    pub items: Vec<EnvItem>,
}

impl EnvReport {
    pub fn from_json(v: &Value) -> EnvReport {
        let items = v
            .get("items")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(EnvItem::from_json).collect())
            .unwrap_or_default();
        let agent = v.get("agent").unwrap_or(&Value::Null);
        EnvReport {
            repo: opt_str(v, "repo").unwrap_or_default(),
            location: opt_str(agent, "location").unwrap_or_default(),
            model: opt_str(agent, "model"),
            identity: v
                .get("github")
                .and_then(|g| opt_str(g, "identity"))
                .unwrap_or_default(),
            items,
        }
    }

    pub fn item(&self, kind: &str, name: &str) -> Option<&EnvItem> {
        self.items.iter().find(|i| i.kind == kind && i.name == name)
    }

    pub fn of_kind(&self, kind: &str) -> Vec<&EnvItem> {
        self.items.iter().filter(|i| i.kind == kind).collect()
    }

    pub fn credential(&self, name: &str) -> Option<&EnvItem> {
        self.item("credential", name)
    }

    pub fn provider(&self) -> Option<&str> {
        self.item("box", "provider")
            .and_then(|i| i.detail.as_deref())
    }

    pub fn size(&self) -> Option<&str> {
        self.item("box", "size").and_then(|i| i.detail.as_deref())
    }
}

impl EnvItem {
    fn from_json(v: &Value) -> EnvItem {
        EnvItem {
            kind: opt_str(v, "kind").unwrap_or_default(),
            name: opt_str(v, "name").unwrap_or_default(),
            source: opt_str(v, "source").unwrap_or_default(),
            detail: opt_str(v, "detail"),
            resolves: v.get("resolves").and_then(Value::as_bool),
            error: opt_str(v, "error"),
            delivery: opt_str(v, "delivery"),
        }
    }
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
