//! clap surface for the v2 command set and its conversion to `Request`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use moor_core::controller::WatchFilter;
use moor_core::request::{EndSessionReq, ShowSessionReq};
use moor_core::{BoxName, Request, RunReq, SessionId, ShowReq};

use super::args_admin::{AdminCmd, GateCmd};
use super::args_probe::Flags;
use super::GlobalOpts;
use ask::AskArgs;
use box_ops::{BoxArg, BoxConfigArgs, CloseArgs, CloseFailedArgs, CpArgs};
use events::EventsArgs;
use exec::ExecArgs;
use forward::ForwardArgs;
use inspect::InspectArgs;
use node::NodeCmd;
use repo::{config_text, RepoCli};
use session::SessionCmd;
use start::StartArgs;
use steer::SteerArgs;
use user::UserCli;
use wait::WaitArgs;
use wait_events::WaitEventsArgs;

mod ask;
mod box_ops;
mod events;
mod exec;
mod forward;
mod inspect;
mod node;
#[cfg(test)]
mod parse_ignoring_env;
mod repo;
mod session;
mod start;
mod steer;
#[cfg(test)]
pub(super) use parse_ignoring_env::parse_ignoring_env;
#[cfg(test)]
mod tests;
mod user;
mod wait;
mod wait_events;

pub(super) use super::NodeAddSpec;

/// What a parsed command line asks for.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "one value per command line, built and matched once; boxing the request would touch every command"
)]
pub(super) enum Action {
    /// A plain request for the library.
    Request(Request),
    /// `call`: read a JSON request from stdin.
    CallStdin,
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
    /// Hidden `box-term --runner <addr> <machine-id> <slug> --place <place> -- <script>`: run a
    /// script in one place of a machine's Box on this terminal.
    BoxTerm(super::BoxTerm),
    /// `exec`: run a command with its stdio streamed; the flag forwards a terminal's stdin.
    Exec(moor_core::ExecReq, bool),
    /// `forward`: serve a Box's port on this computer's loopback until interrupted.
    Forward(moor_client::port_forward::Spec),
    /// A request that carries a secret read from stdin, never from argv.
    Stdin(super::StdinRequest),
    /// `doctor`: check the setup and print one line per check.
    Doctor {
        /// Public URL of the gate, to check it answers.
        public_url: Option<String>,
    },
    /// `up`: set this host up as a controller with a local node.
    Up,
    /// `down`: stop the controller's user service and remove it.
    Down {
        /// Stop it although Boxes are open and running.
        force: bool,
    },
    /// `node add`: enroll a machine agent on this host as a shared node.
    NodeAdd(NodeAddSpec),
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
    /// Create or attach a Box to work in by hand: it runs the repo's setup and no agent.
    Start(StartArgs),
    /// Start a task, or ask a question, about some repos; an agent place starts for it.
    Ask(AskArgs),
    /// Run a command in the Box.
    Run(RunArgs),
    /// Run a command in the Box with stdin, stdout and stderr passed byte for byte.
    Exec(ExecArgs),
    /// Serve a port of the Box on this computer's `localhost` until Ctrl-C (dev tool).
    Forward(ForwardArgs),
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
    /// Replace a Box's config override, or remove it; the Box's next exec, run, setup line and
    /// teardown read it.
    BoxConfig(BoxConfigArgs),
    /// Copy a file to or from the Box.
    Cp(CpArgs),
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
    /// A repo's stored settings.
    #[command(subcommand)]
    Repo(RepoCli),
    /// Set this host up: controller service, local node, your user and its secrets.
    Up,
    /// Stop and remove the controller service; refuses while Boxes run.
    Down(DownArgs),
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
            Self::Ask(a) => Action::Request(Request::Ask(a.into())),
            Self::Run(a) => Action::Request(Request::Run(a.into())),
            Self::Forward(a) => Action::Forward(a.into()),
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
            Self::BoxConfig(a) => Action::Request(Request::BoxConfigSet(a.into())),
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
            Self::Node(n) => n.into_action(),
            Self::Repo(r) => Action::Request(r.into_request()),
            Self::Up => Action::Up,
            Self::Down(a) => Action::Down { force: a.force },
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

/// Arguments of `down`.
#[derive(Debug, Args)]
struct DownArgs {
    /// Stop the services although Boxes are open and running.
    #[arg(long)]
    force: bool,
}

/// Arguments of `doctor`.
#[derive(Debug, Args)]
struct DoctorArgs {
    /// Public URL of the gate (`moor-gate --public-url`), to check it answers.
    #[arg(long, value_name = "URL")]
    public_url: Option<String>,
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
            transcript: false,
            everyone: false,
        }
    }
}
