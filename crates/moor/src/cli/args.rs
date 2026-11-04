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
use box_ops::{BoxArg, CloseArgs, CloseFailedArgs, CpArgs};
use events::EventsArgs;
use exec::ExecArgs;
use inspect::InspectArgs;
use node::NodeCmd;
use session::SessionCmd;
use user::UserCli;
use wait::WaitArgs;
use wait_events::WaitEventsArgs;

mod box_ops;
mod events;
mod exec;
mod inspect;
mod node;
#[cfg(test)]
mod parse_ignoring_env;
mod session;
#[cfg(test)]
pub(super) use parse_ignoring_env::parse_ignoring_env;
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
    /// `store migrate`: bring the store's schema to this build's latest migration.
    StoreMigrate,
    /// `store copy`: copy the `SQLite` store into an empty Postgres database.
    StoreCopy {
        /// The Postgres URL to copy into.
        to: String,
    },
    /// Hidden `link-dial <machine-id> <slug>`: bridge stdio to a pool lease's sshd.
    LinkDial {
        /// The pool host's machine id.
        machine_id: i64,
        /// The lease.
        slug: String,
    },
    /// `exec`: run a command with its stdio streamed; the flag forwards a terminal's stdin.
    Exec(moor_core::ExecReq, bool),
    /// A request that carries a secret read from stdin, never from argv.
    Stdin(super::StdinRequest),
    /// `doctor`: check the setup and print one line per check.
    Doctor {
        /// Public URL of the gate, to check it answers.
        public_url: Option<String>,
    },
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
    /// Run a command in the Box with stdin, stdout and stderr passed byte for byte.
    Exec(ExecArgs),
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
    /// Look at one Box to tell working from stuck.
    Inspect(InspectArgs),
    /// Close a Box.
    Close(CloseArgs),
    /// Close every Box whose setup failed.
    CloseFailed(CloseFailedArgs),
    /// Copy a file to or from the Box.
    Cp(CpArgs),
    /// Focus the Box's Workspace.
    Open(BoxArg),
    /// Put the Box to sleep.
    Sleep(BoxArg),
    /// Wake a sleeping Box.
    Wake(BoxArg),
    /// Re-run setup on a Box whose setup failed.
    Retry(BoxArg),
    /// Check a gate; record one with `moor run --record <name>`.
    #[command(subcommand)]
    Gate(GateCmd),
    /// Print stored events as JSON lines; --follow keeps streaming (dev tool).
    Events(EventsArgs),
    /// Your moor user and its secrets (dev tool).
    #[command(subcommand)]
    User(UserCli),
    /// The nodes Boxes run on.
    #[command(subcommand)]
    Node(NodeCmd),
    /// Check the setup after install and say how to fix what is wrong.
    Doctor(DoctorArgs),
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
            Self::Exec(a) => {
                let (req, interactive) = a.into_parts();
                Action::Exec(req, interactive)
            }
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
            Self::Inspect(a) => Action::Request(Request::Inspect(a.into())),
            Self::Close(a) => Action::Request(Request::Close(a.into())),
            Self::CloseFailed(a) => Action::Request(Request::CloseFailed(a.into())),
            Self::Cp(a) => Action::Request(Request::Cp(a.into())),
            Self::Open(a) => Action::Request(Request::Open(a.into())),
            Self::Sleep(a) => Action::Request(Request::Sleep(a.into())),
            Self::Wake(a) => Action::Request(Request::Wake(a.into())),
            Self::Retry(a) => Action::Request(Request::Retry(a.into())),
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
            Self::Node(n) => Action::Request(n.into()),
            Self::Doctor(a) => Action::Doctor {
                public_url: a.public_url,
            },
            Self::Whoami => Action::Request(Request::Whoami(moor_core::request::AsReq {
                identity: None,
            })),
            Self::Admin(a) => a.into_action(),
        }
    }
}

/// Arguments of `doctor`.
#[derive(Debug, Args)]
struct DoctorArgs {
    /// Public URL of the gate (`moor-gate --public-url`), to check it answers.
    #[arg(long, value_name = "URL")]
    public_url: Option<String>,
}

/// Arguments of `start`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools, reason = "one field per CLI flag")]
struct StartArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Prompt for the agent.
    #[arg(long)]
    prompt: Option<String>,
    /// Issue reference.
    #[arg(long)]
    issue: Option<String>,
    /// Base ref.
    #[arg(long)]
    base: Option<String>,
    /// Repo to clone, as OWNER/NAME or a github.com URL; the current checkout's origin when absent.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::caller::repo_arg)]
    repo: Option<moor_core::seam::forge::ForgeRepo>,
    /// Another repo to clone beside `--repo`, as OWNER/NAME or a github.com URL; repeat for
    /// more. Any use replaces `box.also` from your config.
    #[arg(long, value_name = "OWNER/NAME|URL", value_parser = super::caller::repo_arg)]
    also: Vec<moor_core::seam::forge::ForgeRepo>,
    /// Agent model.
    #[arg(long)]
    model: Option<String>,
    /// A node of yours to run the Box on, in place of your default; see `moor node ls`.
    #[arg(long, value_name = "NODE")]
    node: Option<String>,
    /// Do not launch an agent.
    #[arg(long)]
    no_agent: bool,
    /// Resume the agent's stored session.
    #[arg(long)]
    resume: bool,
    /// Parent Session id; defaults to the `MOOR_SESSION` of the calling agent.
    #[arg(long, value_name = "ID", env = "MOOR_SESSION")]
    parent: Option<i64>,
}

impl From<StartArgs> for StartReq {
    fn from(a: StartArgs) -> Self {
        Self {
            name: a.name,
            prompt: a.prompt,
            issue: a.issue,
            base: a.base,
            repo: a.repo,
            model: a.model,
            node: a.node,
            no_agent: a.no_agent,
            resume: a.resume,
            parent: a.parent.map(SessionId::new),
            also: (!a.also.is_empty()).then_some(a.also),
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
            box_name: a.name,
            record: a.record,
            argv: a.cmd,
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
    #[arg(conflicts_with_all = ["menu", "interrupt"])]
    text: Option<String>,
    /// Menu choice to send.
    #[arg(
        long,
        required_unless_present_any = ["text", "interrupt"],
        conflicts_with = "interrupt",
        value_name = "N"
    )]
    menu: Option<u32>,
    /// Send the agent's interrupt key, which stops its current work.
    #[arg(long)]
    interrupt: bool,
}

impl From<SteerArgs> for SteerReq {
    fn from(a: SteerArgs) -> Self {
        Self {
            target: a.name.into(),
            text: a.text,
            menu: a.menu,
            interrupt: a.interrupt,
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
