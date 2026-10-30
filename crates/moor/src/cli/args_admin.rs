//! clap definitions for the admin verbs: gate, env, proxy, controller, call, version.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use moor_core::{BoxId, EnvReq, GateReq, Request};

use super::args::Action;

/// Admin verbs, flattened into the top-level command.
#[derive(Debug, Subcommand)]
pub(super) enum AdminCmd {
    /// Preview the layered agent environment.
    Env(EnvArgs),
    /// Proxy commands.
    #[command(subcommand)]
    Proxy(ProxyCmd),
    /// Controller commands.
    #[command(subcommand)]
    Controller(ControllerCmd),
    /// Bridge stdin/stdout to a pool lease's sshd over its host's link, as an ssh
    /// `ProxyCommand`.
    #[command(hide = true)]
    LinkDial(LinkDialArgs),
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

/// Subcommands of `proxy`.
#[derive(Debug, Subcommand)]
pub(super) enum ProxyCmd {
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

/// Arguments of `link-dial`.
#[derive(Debug, Args)]
pub(super) struct LinkDialArgs {
    /// The pool host's machine id.
    machine_id: i64,
    /// The lease, `moor-<id>`.
    slug: String,
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
            Self::Env(a) => Action::Request(Request::Env(EnvReq {
                repo: a.repo,
                base: a.base,
            })),
            Self::Proxy(ProxyCmd::Serve(a)) => Action::ProxyServe(BoxId::new(a.box_id)),
            Self::Controller(ControllerCmd::Serve(a)) => Action::ControllerServe(a.config),
            Self::Controller(ControllerCmd::Attach(a)) => Action::ControllerAttach(a.socket),
            Self::LinkDial(a) => Action::LinkDial {
                machine_id: a.machine_id,
                slug: a.slug,
            },
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
