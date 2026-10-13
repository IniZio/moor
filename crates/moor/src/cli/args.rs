//! clap surface for the v2 command set and its conversion to `Request`.

use std::ffi::OsString;

use clap::{Args, Parser, Subcommand, ValueEnum};
use moor_core::{
    CloseReq, CpReq, RemoveReq, ReportReq, Request, RunReq, ShowReq, StartReq, SteerReq, TaskName,
    TaskReq, WaitFor, WaitReq,
};

use super::args_admin::{AdminCmd, GateCmd};
use super::args_probe::Flags;
use super::GlobalOpts;

#[cfg(test)]
mod tests;

/// What a parsed command line asks for.
#[derive(Debug)]
pub(super) enum Action {
    /// A plain request for the library.
    Request(Request),
    /// `call`: read a JSON request from stdin.
    CallStdin,
    /// Hidden `proxy serve`.
    ProxyServe,
}

/// Parse argv (program name first) into global options and an action.
pub(super) fn parse(args: Vec<OsString>) -> Result<(GlobalOpts, Action), clap::Error> {
    let cli = Cli::try_parse_from(args)?;
    Ok((cli.flags.into(), cli.command.into_action()))
}

/// clap parser for a Task name.
fn task_name(s: &str) -> Result<TaskName, String> {
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
    /// Record or check a gate.
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
    /// Agent model.
    #[arg(long)]
    model: Option<String>,
    /// Do not launch an agent.
    #[arg(long)]
    no_agent: bool,
    /// Resume the agent's stored session.
    #[arg(long)]
    resume: bool,
}

impl From<StartArgs> for StartReq {
    fn from(a: StartArgs) -> Self {
        Self {
            name: a.name,
            goal: a.goal,
            issue: a.issue,
            worktree: a.worktree,
            base: a.base,
            model: a.model,
            no_agent: a.no_agent,
            resume: a.resume,
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
    /// Record the run under this name.
    #[arg(long, value_name = "NAME")]
    record: Option<String>,
    /// Command and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

impl From<RunArgs> for RunReq {
    fn from(a: RunArgs) -> Self {
        Self {
            task: a.task,
            no_sync: a.no_sync,
            record: a.record,
            argv: a.cmd,
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
    /// Timeout in seconds.
    #[arg(long, value_name = "S")]
    timeout: Option<u64>,
}

impl From<WaitArgs> for WaitReq {
    fn from(a: WaitArgs) -> Self {
        let wait_for = a.wait_for.map(|w| match w {
            WaitForArg::Idle => WaitFor::Idle,
            WaitForArg::Done => WaitFor::Done,
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

/// Arguments of `close`.
#[derive(Debug, Args)]
struct CloseArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Close despite unsaved work.
    #[arg(long)]
    force: bool,
}

impl From<CloseArgs> for CloseReq {
    fn from(a: CloseArgs) -> Self {
        Self {
            task: a.task,
            force: a.force,
        }
    }
}

/// Arguments of `remove`.
#[derive(Debug, Args)]
struct RemoveArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Discard the Task's work.
    #[arg(long)]
    discard: bool,
}

impl From<RemoveArgs> for RemoveReq {
    fn from(a: RemoveArgs) -> Self {
        Self {
            task: a.task,
            discard: a.discard,
        }
    }
}

/// Arguments of `cp`.
#[derive(Debug, Args)]
struct CpArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
    /// Source path in the Box; relative to its workdir and may not contain
    /// `..`. Use an absolute Box path to reach outside the workdir.
    src: String,
    /// Destination path on the host; relative to the current directory.
    dest: String,
}

impl From<CpArgs> for CpReq {
    fn from(a: CpArgs) -> Self {
        Self {
            task: a.task,
            src: a.src,
            dest: a.dest,
        }
    }
}

/// A lone Task argument.
#[derive(Debug, Args)]
struct TaskArg {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: TaskName,
}

impl From<TaskArg> for TaskReq {
    fn from(a: TaskArg) -> Self {
        Self { task: a.task }
    }
}
