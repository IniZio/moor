//! clap surface for the v2 command set and its conversion to `Request`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use moor_core::controller::WatchFilter;
use moor_core::request::{EndSessionReq, ShowSessionReq};
use moor_core::{BoxId, BoxName, Request, RunReq, SessionId, ShowReq, StartReq, SteerReq};

use super::args_admin::{AdminCmd, GateCmd};
use super::args_probe::Flags;
use super::GlobalOpts;
use box_ops::{BoxArg, CloseArgs, CpArgs};
use events::EventsArgs;
use session::SessionCmd;
use user::UserCli;
use wait::WaitArgs;
use wait_events::WaitEventsArgs;

mod box_ops;
mod events;
mod session;
#[cfg(test)]
mod tests;
mod user;
mod wait;
mod wait_events;

/// What a parsed command line asks for.
#[derive(Debug)]
pub(super) enum Action {
    /// A plain request for the library.
    Request(Request),
    /// `call`: read a JSON request from stdin.
    CallStdin,
    /// Hidden `proxy serve <box_id>`: serve the credential proxy for that Box.
    ProxyServe(BoxId),
    /// `controller serve --config <file>`: run the controller daemon.
    ControllerServe(PathBuf),
    /// `controller attach [--socket <path>]`: bridge stdio to the controller socket.
    ControllerAttach(Option<PathBuf>),
    /// `user secret set <name>`: read the value from stdin and store it.
    SecretSetStdin(moor_core::user::SecretName),
    /// `events`: print the event stream as JSON lines.
    Events {
        /// Only events after this id.
        since: i64,
        /// Which events to print.
        filter: WatchFilter,
        /// Keep streaming.
        follow: bool,
    },
}

/// Parse argv (program name first) into global options and an action.
pub(super) fn parse(args: Vec<OsString>) -> Result<(GlobalOpts, Action), clap::Error> {
    let cli = Cli::try_parse_from(args)?;
    Ok((cli.flags.into(), cli.command.into_action()))
}

/// `parse` with the `MOOR_SESSION` default of `--parent` ignored, so tests do not
/// depend on the process environment.
#[cfg(test)]
pub(super) fn parse_ignoring_env(args: Vec<OsString>) -> Result<(GlobalOpts, Action), clap::Error> {
    use clap::{CommandFactory, FromArgMatches};
    fn strip(cmd: clap::Command) -> clap::Command {
        cmd.mut_args(|a| {
            if a.get_id() == "parent" {
                a.env(None::<&str>)
            } else {
                a
            }
        })
        .mut_subcommands(strip)
    }
    let matches = strip(Cli::command()).try_get_matches_from(args)?;
    let cli = Cli::from_arg_matches(&matches)?;
    Ok((cli.flags.into(), cli.command.into_action()))
}

/// clap parser for a Box name.
pub(super) fn box_name(s: &str) -> Result<BoxName, String> {
    BoxName::parse_allow_bare(s).map_err(|e| e.to_string())
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
    /// Create or attach a Box.
    Start(StartArgs),
    /// Run a command in the Box.
    Run(RunArgs),
    /// Wait for a Box state.
    Wait(WaitArgs),
    /// Wait for events from several Boxes.
    WaitEvents(WaitEventsArgs),
    /// Send text or a menu choice to the agent.
    Steer(SteerArgs),
    /// End the agent Session; keep the Box open.
    End(BoxArg),
    /// Agent Session verbs.
    #[command(subcommand)]
    Session(SessionCmd),
    /// Show one Box or all open Boxes.
    #[command(visible_alias = "list")]
    Show(ShowArgs),
    /// Close a Box.
    Close(CloseArgs),
    /// Copy a file to or from the Box.
    Cp(CpArgs),
    /// Focus the Box's Workspace.
    Open(BoxArg),
    /// Put the Box to sleep.
    Sleep(BoxArg),
    /// Wake a sleeping Box.
    Wake(BoxArg),
    /// Check a gate; record one with `moor run --record <name>`.
    #[command(subcommand)]
    Gate(GateCmd),
    /// Print stored events as JSON lines; --follow keeps streaming (dev tool).
    Events(EventsArgs),
    /// Your moor user, its secrets and its link codes (dev tool).
    #[command(subcommand)]
    User(UserCli),
    /// Show your identity, its user and that user's linked identities.
    Whoami,
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
            Self::WaitEvents(a) => Action::Request(Request::WaitEvents(a.into())),
            Self::Steer(a) => Action::Request(Request::Steer(a.into())),
            Self::End(a) => Action::Request(Request::EndSession(EndSessionReq {
                box_name: a.name.as_str().to_string(),
            })),
            Self::Session(c) => Action::Request(c.into()),
            Self::Show(ShowArgs {
                session: Some(session),
                ..
            }) => Action::Request(Request::ShowSession(ShowSessionReq {
                session: SessionId::new(session),
            })),
            Self::Show(a) => Action::Request(Request::Show(a.into())),
            Self::Close(a) => Action::Request(Request::Close(a.into())),
            Self::Cp(a) => Action::Request(Request::Cp(a.into())),
            Self::Open(a) => Action::Request(Request::Open(a.into())),
            Self::Sleep(a) => Action::Request(Request::Sleep(a.into())),
            Self::Wake(a) => Action::Request(Request::Wake(a.into())),
            Self::Gate(g) => Action::Request(Request::Gate(g.into())),
            Self::Events(a) => {
                let (since, filter, follow) = a.into_parts();
                Action::Events {
                    since,
                    filter,
                    follow,
                }
            }
            Self::User(u) => u.into_action(),
            Self::Whoami => Action::Request(Request::Whoami(moor_core::request::AsReq {
                identity: None,
            })),
            Self::Admin(a) => a.into_action(),
        }
    }
}

/// Arguments of `start`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools, reason = "one field per CLI flag")]
struct StartArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
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
    /// Launch the agent with no first prompt.
    #[arg(long, conflicts_with = "no_agent")]
    hold: bool,
    /// Resume the agent's stored session.
    #[arg(long)]
    resume: bool,
    /// Launch the agent with the Box's .moor.yaml even when it differs from its base commit.
    #[arg(long)]
    trust_moor_yaml: bool,
    /// Parent Session id; defaults to the `MOOR_SESSION` of the calling agent.
    #[arg(long, value_name = "ID", env = "MOOR_SESSION")]
    parent: Option<i64>,
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
            hold: a.hold,
            resume: a.resume,
            trust_moor_yaml: a.trust_moor_yaml,
            parent: a.parent.map(SessionId::new),
            identity: None,
        }
    }
}

/// Arguments of `run`.
#[derive(Debug, Args)]
struct RunArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Skip the pre-run sync.
    #[arg(long)]
    no_sync: bool,
    /// Sync before the run, after checking the Box holds no unsaved work.
    #[arg(long, conflicts_with = "no_sync")]
    sync: bool,
    /// Record the run under this name.
    #[arg(long, value_name = "NAME")]
    record: Option<String>,
    /// Run even when the Box's .moor.yaml differs from its base commit.
    #[arg(long)]
    trust_moor_yaml: bool,
    /// Command and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

impl From<RunArgs> for RunReq {
    fn from(a: RunArgs) -> Self {
        Self {
            box_name: a.name,
            no_sync: a.no_sync,
            sync: a.sync,
            record: a.record,
            argv: a.cmd,
            trust_moor_yaml: a.trust_moor_yaml,
            inherit_output: false,
        }
    }
}

/// Arguments of `steer`.
#[derive(Debug, Args)]
struct SteerArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
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
            target: a.name.into(),
            text: a.text,
            menu: a.menu,
        }
    }
}

/// Arguments of `show`.
#[derive(Debug, Args)]
struct ShowArgs {
    /// Box name; all open Boxes when absent.
    #[arg(value_parser = box_name)]
    name: Option<BoxName>,
    /// Show one Session (host or Box) by id, with its child Sessions.
    #[arg(
        long,
        value_name = "ID",
        conflicts_with_all = ["name", "events", "agent", "reports", "pane"]
    )]
    session: Option<i64>,
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
            target: a.name.map(Into::into),
            events: a.events,
            agent: a.agent,
            reports: a.reports,
            artifacts: false,
            pane: a.pane,
        }
    }
}
