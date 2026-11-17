//! clap definitions for the admin verbs: gate, env, proxy, controller, call, version.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use moor_core::{EnvReq, GateReq, Request};

use super::args::Action;

/// Admin verbs, flattened into the top-level command.
#[derive(Debug, Subcommand)]
pub(super) enum AdminCmd {
    /// Preview the layered agent environment.
    Env(EnvArgs),
    /// Controller commands.
    #[command(subcommand)]
    Controller(ControllerCmd),
    /// State store commands.
    #[command(subcommand)]
    Store(StoreCmd),
    /// Open a terminal in one place of a Box that a machine provides, through its runner's links
    /// broker, as the argv a pane runs.
    #[command(hide = true)]
    BoxTerm(BoxTermArgs),
    /// Run a JSON request read from stdin.
    Call,
    /// Print version information.
    Version,
}

/// Arguments of `env`.
#[derive(Debug, Args)]
pub(super) struct EnvArgs {
    /// Repo to preview, as OWNER/NAME or a github.com URL; the current checkout's when absent.
    #[arg(value_parser = super::caller::repo_arg, value_name = "OWNER/NAME|URL")]
    repo: Option<moor_core::seam::forge::ForgeRepo>,
    /// Commit or ref to read the repo at; the default branch when absent.
    #[arg(long)]
    base: Option<String>,
}

/// Subcommands of `controller`.
#[derive(Debug, Subcommand)]
pub(super) enum ControllerCmd {
    /// Serve moor requests on a local socket.
    Serve(ControllerServeArgs),
    /// Bridge stdin/stdout to the controller socket (used over ssh).
    Attach(ControllerAttachArgs),
}

/// Subcommands of `store`.
#[derive(Debug, Subcommand)]
pub(super) enum StoreCmd {
    /// Bring the store's schema to this moor's latest migration, then exit.
    Migrate,
    /// Copy the SQLite store into an empty Postgres database, then exit.
    Copy(StoreCopyArgs),
}

/// Arguments of `store copy`.
#[derive(Debug, Args)]
pub(super) struct StoreCopyArgs {
    /// Postgres URL of the empty database to copy into.
    #[arg(long, value_name = "URL")]
    to: String,
}

/// Arguments of `controller attach`.
#[derive(Debug, Args)]
pub(super) struct ControllerAttachArgs {
    /// Controller socket path.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
}

/// Arguments of `box-term`.
#[derive(Debug, Args)]
pub(super) struct BoxTermArgs {
    /// The runner that holds the machine's link: its socket path, or `host:port`.
    #[arg(long, value_name = "ADDR")]
    runner: String,
    /// The machine's id.
    machine_id: i64,
    /// The Box, `moor-<controller>-<id>`.
    slug: String,
    /// The place in the Box.
    #[arg(long, value_enum)]
    place: PlaceArg,
    /// The shell script to run there.
    #[arg(last = true, required = true, num_args = 1)]
    script: Vec<String>,
}

/// A place in a Box, as `box-term` takes it.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(super) enum PlaceArg {
    /// Where the agent runs.
    Agent,
    /// Where repos build.
    Work,
}

/// Arguments of `controller serve`.
#[derive(Debug, Args)]
pub(super) struct ControllerServeArgs {
    /// Controller config file.
    #[arg(long)]
    config: PathBuf,
}

impl AdminCmd {
    /// Convert the parsed admin verb into an action.
    pub(super) fn into_action(self) -> Action {
        match self {
            Self::Env(a) => Action::Request(Request::Env(EnvReq {
                repo: a.repo,
                base: a.base,
            })),
            Self::Controller(ControllerCmd::Serve(a)) => Action::ControllerServe(a.config),
            Self::Controller(ControllerCmd::Attach(a)) => Action::ControllerAttach(a.socket),
            Self::Store(StoreCmd::Migrate) => Action::StoreMigrate,
            Self::Store(StoreCmd::Copy(a)) => Action::StoreCopy { to: a.to },
            Self::BoxTerm(a) => Action::BoxTerm(super::BoxTerm {
                runner: a.runner,
                machine_id: a.machine_id,
                slug: a.slug,
                place: match a.place {
                    PlaceArg::Agent => moor_core::seam::driver::Place::Agent,
                    PlaceArg::Work => moor_core::seam::driver::Place::Work,
                },
                script: a.script.concat(),
            }),
            Self::Call => Action::CallStdin,
            Self::Version => Action::Request(Request::Version {}),
        }
    }
}

/// Subcommands of `gate`.
#[derive(Debug, Subcommand)]
pub(super) enum GateCmd {
    /// Check a gate for a commit.
    Check(GateArgs),
}

/// Arguments of `gate check`.
#[derive(Debug, Args)]
pub(super) struct GateArgs {
    /// Commit sha.
    sha: String,
    /// Gate name.
    name: String,
    /// Command to run for the gate.
    #[arg(long)]
    command: Option<String>,
}

impl From<GateCmd> for GateReq {
    fn from(g: GateCmd) -> Self {
        let GateCmd::Check(a) = g;
        Self {
            sha: a.sha,
            name: a.name,
            command: a.command,
        }
    }
}
