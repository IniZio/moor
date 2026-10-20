//! clap definitions for the admin verbs: gate, env, github, proxy, controller, call, version.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use moor_core::{BoxId, EnvReq, GateReq, Request};

use super::args::Action;

/// Admin verbs, flattened into the top-level command.
#[derive(Debug, Subcommand)]
pub(super) enum AdminCmd {
    /// Preview the layered agent environment.
    Env(EnvArgs),
    /// GitHub credential setup.
    #[command(subcommand)]
    Github(GithubCmd),
    /// Proxy commands.
    #[command(subcommand)]
    Proxy(ProxyCmd),
    /// Controller commands.
    #[command(subcommand)]
    Controller(ControllerCmd),
    /// Run a JSON request read from stdin.
    Call,
    /// Print version information.
    Version,
}

/// Arguments of `env`.
#[derive(Debug, Args)]
pub(super) struct EnvArgs {
    /// Repo to preview; the current one when absent.
    repo: Option<String>,
}

/// Subcommands of `github`.
#[derive(Debug, Subcommand)]
pub(super) enum GithubCmd {
    /// Set up GitHub access.
    Setup,
    /// Log in to GitHub.
    Login,
}

/// Subcommands of `proxy`.
#[derive(Debug, Subcommand)]
pub(super) enum ProxyCmd {
    /// Self-test each proxy route.
    Check,
    /// Serve the credential proxy.
    #[command(hide = true)]
    Serve(ServeArgs),
}

/// Subcommands of `controller`.
#[derive(Debug, Subcommand)]
pub(super) enum ControllerCmd {
    /// Serve moor requests on a local socket.
    Serve(ControllerServeArgs),
    /// Bridge stdin/stdout to the controller socket (used over ssh).
    Attach(ControllerAttachArgs),
}

/// Arguments of `controller attach`.
#[derive(Debug, Args)]
pub(super) struct ControllerAttachArgs {
    /// Controller socket path.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
}

/// Arguments of `controller serve`.
#[derive(Debug, Args)]
pub(super) struct ControllerServeArgs {
    /// Controller config file.
    #[arg(long)]
    config: PathBuf,
}

/// Arguments of `proxy serve`.
#[derive(Debug, Args)]
pub(super) struct ServeArgs {
    /// Box id to serve.
    box_id: i64,
}

impl AdminCmd {
    /// Convert the parsed admin verb into an action.
    pub(super) fn into_action(self) -> Action {
        match self {
            Self::Env(a) => Action::Request(Request::Env(EnvReq { repo: a.repo })),
            Self::Github(GithubCmd::Setup) => Action::Request(Request::GithubSetup {}),
            Self::Github(GithubCmd::Login) => Action::Request(Request::GithubLogin {}),
            Self::Proxy(ProxyCmd::Check) => Action::Request(Request::ProxyCheck {}),
            Self::Proxy(ProxyCmd::Serve(a)) => Action::ProxyServe(BoxId::new(a.box_id)),
            Self::Controller(ControllerCmd::Serve(a)) => Action::ControllerServe(a.config),
            Self::Controller(ControllerCmd::Attach(a)) => Action::ControllerAttach(a.socket),
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
