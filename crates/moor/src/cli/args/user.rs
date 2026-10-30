//! clap definitions for `moor user`: the caller's user and its secrets.

use clap::{Args, Subcommand};
use moor_core::config::OAuthCallback;
use moor_core::name::ProfileSource;
use moor_core::request::{
    AsReq, EnrollReq, MachineAddReq, MachineRemoveReq, McpAddReq, McpOAuthFinishReq, McpRemoveReq,
    ProfileSetReq, SecretDeleteReq,
};
use moor_core::user::SecretName;
use moor_core::Request;

use super::Action;

/// Subcommands of `user`; each acts as the caller's `local:<uid>` identity.
#[derive(Debug, Subcommand)]
pub(super) enum UserCli {
    /// Create your user, or show the one you have.
    Enroll,
    /// Your user's secrets.
    #[command(subcommand)]
    Secret(SecretCli),
    /// Your user's own MCP servers; each server's key is the secret `mcp_<name>`.
    #[command(subcommand)]
    Mcp(McpCli),
    /// Your user's own machines, reached over ssh with this host's own key.
    #[command(subcommand)]
    Machine(MachineCli),
    /// The git repo that is your agent profile.
    #[command(subcommand)]
    Profile(ProfileCli),
}

/// Subcommands of `user profile`.
#[derive(Debug, Subcommand)]
pub(super) enum ProfileCli {
    /// Record `https://github.com/OWNER/NAME[#REF]` as your profile. Without `#REF` it tracks
    /// the repo's default branch.
    Set(ProfileSetArg),
    /// Show your profile repo.
    Show,
    /// Clear your profile repo.
    Rm,
}

/// Arguments of `user profile set`.
#[derive(Debug, Args)]
pub(super) struct ProfileSetArg {
    /// The repo, as `https://github.com/OWNER/NAME`, then `#` and a branch, tag or sha to pin.
    #[arg(value_parser = ProfileSource::parse)]
    source: ProfileSource,
}

/// Subcommands of `user secret`.
#[derive(Debug, Subcommand)]
pub(super) enum SecretCli {
    /// Store secret NAME; the value is read from stdin, never from arguments.
    Set(NameArg),
    /// Delete secret NAME.
    Rm(NameArg),
    /// List your secrets' names.
    Ls,
}

/// Subcommands of `user mcp`.
#[derive(Debug, Subcommand)]
pub(super) enum McpCli {
    /// Add or replace server NAME at URL; set its key with `moor user secret set mcp_<name>`,
    /// or open the printed `sign_in` address when the server asks for OAuth.
    Add(McpAddArgs),
    /// Finish an OAuth sign-in with the address the browser ended on.
    Finish(McpFinishArg),
    /// Remove server NAME and its key.
    Rm(McpNameArg),
    /// List your servers.
    Ls,
}

/// Arguments of `user mcp finish`.
#[derive(Debug, Args)]
pub(super) struct McpFinishArg {
    /// The full address in the browser's address bar after signing in.
    #[arg(value_parser = OAuthCallback::from_url)]
    pub(super) url: OAuthCallback,
}

/// Subcommands of `user machine`.
#[derive(Debug, Subcommand)]
pub(super) enum MachineCli {
    /// Add or replace machine NAME; no credential is stored.
    Add(MachineAddArgs),
    /// Remove machine NAME.
    Rm(McpNameArg),
    /// List your machines.
    Ls,
}

/// Arguments of `user machine add`.
#[derive(Debug, Args)]
pub(super) struct MachineAddArgs {
    /// Machine name, such as `mac`.
    name: String,
    /// Target as `user@host` or `user@host:port`.
    #[arg(long)]
    ssh: String,
    /// A label such as `macos`; repeat for more.
    #[arg(long = "label")]
    labels: Vec<String>,
    /// How many Boxes may hold the machine at once; slot k moves each port up by 10·k.
    #[arg(long, default_value_t = 1)]
    capacity: u16,
}

/// Arguments of `user mcp add`.
#[derive(Debug, Args)]
pub(super) struct McpAddArgs {
    /// Server name, such as `linear`.
    name: String,
    /// Server URL, such as `https://mcp.linear.app/mcp/readonly`.
    url: String,
    /// Header the key goes in.
    #[arg(long)]
    header: Option<String>,
}

/// A server's name.
#[derive(Debug, Args)]
pub(super) struct McpNameArg {
    /// Server name.
    name: String,
}

/// A secret's name.
#[derive(Debug, Args)]
pub(super) struct NameArg {
    /// Secret name, such as `claude`.
    #[arg(value_parser = SecretName::parse)]
    name: SecretName,
}

/// The `local:<uid>` identity is the socket's own, so no request asserts one.
const AS_CALLER: AsReq = AsReq { identity: None };

impl UserCli {
    /// Convert the parsed verb into an action.
    pub(super) fn into_action(self) -> Action {
        Action::Request(match self {
            Self::Enroll => Request::Enroll(EnrollReq {
                identity: None,
                email: None,
            }),
            Self::Secret(SecretCli::Set(a)) => return Action::SecretSetStdin(a.name),
            Self::Secret(SecretCli::Rm(a)) => Request::SecretDelete(SecretDeleteReq {
                identity: None,
                name: a.name,
            }),
            Self::Secret(SecretCli::Ls) => Request::SecretNames(AS_CALLER),
            Self::Mcp(McpCli::Add(a)) => Request::McpAdd(McpAddReq {
                identity: None,
                name: a.name,
                url: a.url,
                header: a.header,
                key: None,
            }),
            Self::Mcp(McpCli::Rm(a)) => Request::McpRemove(McpRemoveReq {
                identity: None,
                name: a.name,
            }),
            Self::Mcp(McpCli::Ls) => Request::McpList(AS_CALLER),
            Self::Mcp(McpCli::Finish(a)) => Request::McpOAuthFinish(McpOAuthFinishReq {
                identity: None,
                state: a.url.state,
                code: a.url.code,
                iss: a.url.iss,
            }),
            Self::Profile(ProfileCli::Set(a)) => Request::ProfileSet(ProfileSetReq {
                identity: None,
                source: a.source,
            }),
            Self::Profile(ProfileCli::Show) => Request::ProfileShow(AS_CALLER),
            Self::Profile(ProfileCli::Rm) => Request::ProfileRemove(AS_CALLER),
            Self::Machine(MachineCli::Add(a)) => Request::MachineAdd(MachineAddReq {
                identity: None,
                name: a.name,
                ssh: a.ssh,
                labels: a.labels,
                capacity: a.capacity,
            }),
            Self::Machine(MachineCli::Rm(a)) => Request::MachineRemove(MachineRemoveReq {
                identity: None,
                name: a.name,
            }),
            Self::Machine(MachineCli::Ls) => Request::MachineList(AS_CALLER),
        })
    }
}
