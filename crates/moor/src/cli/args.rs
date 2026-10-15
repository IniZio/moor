//! clap surface for the v2 command set and its conversion to `Request`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use moor_core::{
    ReportReq, Request, RunReq, ShowReq, StartReq, SteerReq, TaskId, TaskName, WaitFor, WaitReq,
};

use super::args_admin::{AdminCmd, GateCmd};
use super::args_probe::Flags;
use super::GlobalOpts;
use task_ops::{CloseArgs, CpArgs, RemoveArgs, TaskArg};

mod task_ops;
#[cfg(test)]
mod tests;

/// What a parsed command line asks for.
#[derive(Debug)]
pub(super) enum Action {
    /// A plain request for the library.
    Request(Request),
    /// `call`: read a JSON request from stdin.
    CallStdin,
    /// Hidden `proxy serve <task>`: serve the credential proxy for that Task.
    ProxyServe(TaskId),
    /// `controller serve --config <file>`: run the controller daemon.
    ControllerServe(PathBuf),
    /// `controller attach [--socket <path>]`: bridge stdio to the controller socket.
    ControllerAttach(Option<PathBuf>),
}

/// Parse argv (program name first) into global options and an action.
pub(super) fn parse(args: Vec<OsString>) -> Result<(GlobalOpts, Action), clap::Error> {
    let cli = Cli::try_parse_from(args)?;
    Ok((cli.flags.into(), cli.command.into_action()))
}

/// clap parser for a Task name.
pub(super) fn task_name(s: &str) -> Result<TaskName, String> {
    TaskName::parse(s).map_err(|e| e.to_string())
}

/// Top-level command line.
#[derive(Debug, Parser)]
#[command(
    name = "moor",
    disable_version_flag = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Global flags.
    #[command(flatten)]
    flags: Flags,
    /// The verb.
    #[command(subcommand)]
    command: Cmd,
}

/// Every verb.
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Create or attach a Task.
    Start(StartArgs),
    /// Run a command in the Task's Box.
    Run(RunArgs),
    /// Wait for a Task state.
    Wait(WaitArgs),
    /// Send text or a menu choice to the agent.
    Steer(SteerArgs),
    /// File a Report.
    Report(ReportArgs),
    /// Show one Task or all open Tasks.
    #[command(visible_alias = "list")]
    Show(ShowArgs),
    /// Close a Task.
    Close(CloseArgs),
    /// Remove a Task.
    Remove(RemoveArgs),
    /// Copy a file to or from the Box.
    Cp(CpArgs),
    /// Focus the Task's Workspace.
    Open(TaskArg),
    /// Open a shell in the Box.
    Shell(TaskArg),
    /// Put the Task to sleep.
    Sleep(TaskArg),
    /// Wake a sleeping Task.
    Wake(TaskArg),
    /// Check a gate; record one with `moor run --record <name>`.
    #[command(subcommand)]
    Gate(GateCmd),
    /// Admin verbs.
    #[command(flatten)]
    Admin(AdminCmd),
}

impl Cmd {
    /// Convert the parsed verb into an action.
    fn into_action(self) -> Action {
        match self {
            Self::Start(a) => Action::Request(Request::Start(a.into())),
            Self::Run(a) => Action::Request(Request::Run(a.into())),
            Self::Wait(a) => Action::Request(Request::Wait(a.into())),
            Self::Steer(a) => Action::Request(Request::Steer(a.into())),
            Self::Report(a) => Action::Request(Request::Report(a.into())),
            Self::Show(a) => Action::Request(Request::Show(a.into())),
            Self::Close(a) => Action::Request(Request::Close(a.into())),
            Self::Remove(a) => Action::Request(Request::Remove(a.into())),
            Self::Cp(a) => Action::Request(Request::Cp(a.into())),
            Self::Open(a) => Action::Request(Request::Open(a.into())),
            Self::Shell(a) => Action::Request(Request::Shell(a.into())),
            Self::Sleep(a) => Action::Request(Request::Sleep(a.into())),
            Self::Wake(a) => Action::Request(Request::Wake(a.into())),
            Self::Gate(g) => Action::Request(Request::Gate(g.into())),
            Self::Admin(a) => a.into_action(),
        }
    }
}

/// Arguments of `start`.
#[derive(Debug, Args)]
struct StartArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    name: TaskName,
    /// Goal for the agent.
    #[arg(long)]
    goal: Option<String>,
    /// Issue reference.
    #[arg(long)]
    issue: Option<String>,
    /// Worktree path.
    #[arg(long)]
    worktree: Option<String>,
    /// Base ref.
    #[arg(long)]
    base: Option<String>,
    /// Repo known to the controller, by name or url; resolved on the controller host instead of the current directory.
    #[arg(long, value_name = "NAME|URL")]
    repo: Option<String>,
    /// Agent model.
    #[arg(long)]
    model: Option<String>,
    /// Do not launch an agent.
    #[arg(long)]
    no_agent: bool,
    /// Resume the agent's stored session.
    #[arg(long)]
    resume: bool,
    /// Launch the agent with the Task's .moor.yaml even when it differs from its base commit.
    #[arg(long)]
    trust_moor_yaml: bool,
}

impl From<StartArgs> for StartReq {
    fn from(a: StartArgs) -> Self {
        Self {
            name: a.name,
            goal: a.goal,
            issue: a.issue,
            worktree: a.worktree,
            base: a.base,
            repo: a.repo,
            model: a.model,
            no_agent: a.no_agent,
            resume: a.resume,
            trust_moor_yaml: a.trust_moor_yaml,
        }
    }
}

/// Arguments of `run`.
#[derive(Debug, Args)]
struct RunArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Skip the pre-run sync.
    #[arg(long)]
    no_sync: bool,
    /// Sync before the run, after checking the Box holds no unsaved work.
    #[arg(long, conflicts_with = "no_sync")]
    sync: bool,
    /// Record the run under this name.
    #[arg(long, value_name = "NAME")]
    record: Option<String>,
    /// Run even when the Task's .moor.yaml differs from its base commit.
    #[arg(long)]
    trust_moor_yaml: bool,
    /// Command and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

impl From<RunArgs> for RunReq {
    fn from(a: RunArgs) -> Self {
        Self {
            task: a.task,
            no_sync: a.no_sync,
            sync: a.sync,
            record: a.record,
            argv: a.cmd,
            trust_moor_yaml: a.trust_moor_yaml,
            inherit_output: false,
        }
    }
}

/// States `wait --for` accepts.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum WaitForArg {
    /// Agent idle.
    Idle,
    /// Task done.
    Done,
    /// Agent needs input, or is idle.
    Input,
    /// Run finished.
    Run,
}

/// Arguments of `wait`.
#[derive(Debug, Args)]
struct WaitArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// State to wait for.
    #[arg(long = "for", value_enum)]
    wait_for: Option<WaitForArg>,
    /// Timeout in seconds (default 1800).
    #[arg(long, value_name = "S")]
    timeout: Option<u64>,
}

impl From<WaitArgs> for WaitReq {
    fn from(a: WaitArgs) -> Self {
        let wait_for = a.wait_for.map(|w| match w {
            WaitForArg::Idle => WaitFor::Idle,
            WaitForArg::Done => WaitFor::Done,
            WaitForArg::Input => WaitFor::Input,
            WaitForArg::Run => WaitFor::Run,
        });
        Self {
            task: a.task,
            wait_for,
            timeout_secs: a.timeout,
        }
    }
}

/// Arguments of `steer`.
#[derive(Debug, Args)]
struct SteerArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Text to send.
    #[arg(conflicts_with = "menu")]
    text: Option<String>,
    /// Menu choice to send.
    #[arg(long, required_unless_present = "text", value_name = "N")]
    menu: Option<u32>,
}

impl From<SteerArgs> for SteerReq {
    fn from(a: SteerArgs) -> Self {
        Self {
            task: a.task,
            text: a.text,
            menu: a.menu,
        }
    }
}

/// Arguments of `report`.
#[derive(Debug, Args)]
struct ReportArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Report kind.
    #[arg(long)]
    kind: String,
    /// Report title.
    #[arg(long)]
    title: Option<String>,
    /// Read the body from this file instead of stdin.
    #[arg(long)]
    file: Option<String>,
}

impl From<ReportArgs> for ReportReq {
    fn from(a: ReportArgs) -> Self {
        Self {
            task: a.task,
            kind: a.kind,
            title: a.title,
            file: a.file,
        }
    }
}

/// Arguments of `show`.
#[derive(Debug, Args)]
struct ShowArgs {
    /// Task name; all open Tasks when absent.
    #[arg(value_parser = task_name)]
    task: Option<TaskName>,
    /// Include the last N events.
    #[arg(long, value_name = "N")]
    events: Option<u32>,
    /// Include agent state.
    #[arg(long)]
    agent: bool,
    /// Include Reports.
    #[arg(long)]
    reports: bool,
    /// Include the pane tail.
    #[arg(long)]
    pane: bool,
}

impl From<ShowArgs> for ShowReq {
    fn from(a: ShowArgs) -> Self {
        Self {
            task: a.task,
            events: a.events,
            agent: a.agent,
            reports: a.reports,
            pane: a.pane,
        }
    }
}
