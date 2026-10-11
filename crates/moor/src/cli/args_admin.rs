//! clap definitions for the admin verbs: gate, env, github, proxy, call, version.

use clap::{Args, Subcommand};
use moor_core::{EnvReq, GateAction, GateReq, Request};

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
    Serve,
}

impl AdminCmd {
    /// Convert the parsed admin verb into an action.
    pub(super) fn into_action(self) -> Action {
        match self {
            Self::Env(a) => Action::Request(Request::Env(EnvReq { repo: a.repo })),
            Self::Github(GithubCmd::Setup) => Action::Request(Request::GithubSetup),
            Self::Github(GithubCmd::Login) => Action::Request(Request::GithubLogin),
            Self::Proxy(ProxyCmd::Check) => Action::Request(Request::ProxyCheck),
            Self::Proxy(ProxyCmd::Serve) => Action::ProxyServe,
            Self::Call => Action::CallStdin,
            Self::Version => Action::Request(Request::Version),
        }
    }
}

/// Subcommands of `gate`.
#[derive(Debug, Subcommand)]
pub(super) enum GateCmd {
    /// Record a gate for a commit.
    Record(GateArgs),
    /// Check a gate for a commit.
    Check(GateArgs),
}

/// Arguments of `gate record|check`.
#[derive(Debug, Args)]
pub(super) struct GateArgs {
    /// Commit sha.
    sha: String,
    /// Gate name.
    #[arg(long)]
    name: String,
}

impl From<GateCmd> for GateReq {
    fn from(g: GateCmd) -> Self {
        let (action, a) = match g {
            GateCmd::Record(a) => (GateAction::Record, a),
            GateCmd::Check(a) => (GateAction::Check, a),
        };
        Self {
            action,
            sha: a.sha,
            name: a.name,
        }
    }
}
