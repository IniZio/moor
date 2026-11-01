//! clap definitions for `moor user`: the caller's user and its secrets.

use clap::{Args, Subcommand};
use moor_core::config::OAuthCallback;
use moor_core::name::ProfileSource;
use moor_core::request::{
    AsReq, EnrollReq, MachineRemoveReq, MachineStatusReq, McpAddReq, McpOAuthFinishReq,
    McpRemoveReq, ProfileSetReq, SecretDeleteReq, WebhookAddReq, WebhookRemoveReq,
    WebhookSetupShowReq,
};
use moor_core::user::SecretName;
use moor_core::Request;

use super::Action;
use crate::cli::StdinRequest;

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
    /// Your user's webhooks, each posting Box events to a URL signed with its own key.
    #[command(subcommand)]
    Webhook(WebhookCli),
}

/// Subcommands of `user webhook`.
#[derive(Debug, Subcommand)]
pub(super) enum WebhookCli {
    /// Add a webhook at URL; it posts nothing until `setup` has shown its key and `finish`
    /// has run.
    Add(WebhookAddArgs),
    /// Show webhook ID's setup, with its HMAC key until it is finished.
    Setup(WebhookIdArg),
    /// Finish setting up webhook ID, reading its header's value, if it has a header, from stdin.
    Finish(WebhookIdArg),
    /// List your webhooks.
    Ls,
    /// Remove webhook ID.
    Rm(WebhookIdArg),
}

/// Arguments of `user webhook add`.
#[derive(Debug, Args)]
pub(super) struct WebhookAddArgs {
    /// Where each event is posted: `https://`, or `http://` on the loopback.
    url: String,
    /// An event kind to post; repeat for several. Without any, the usual attention events.
    #[arg(long = "event", value_name = "KIND")]
    events: Vec<String>,
    /// A header the receiver wants its own token in: `Authorization` or an `X-` name.
    #[arg(long, value_name = "NAME")]
    header: Option<String>,
}

/// A webhook's id.
#[derive(Debug, Args)]
pub(super) struct WebhookIdArg {
    /// Webhook id, as `ls` shows it.
    id: i64,
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
    /// Remove machine NAME.
    Rm(McpNameArg),
    /// List your machines.
    Ls,
    /// Ask machine NAME's agent for its version and state; it only reads.
    Status(McpNameArg),
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
            Self::Secret(SecretCli::Set(a)) => {
                return Action::Stdin(StdinRequest::SecretSet(a.name))
            }
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
            Self::Webhook(cli) => return cli.into_action(),
            Self::Machine(MachineCli::Rm(a)) => Request::MachineRemove(MachineRemoveReq {
                identity: None,
                name: a.name,
            }),
            Self::Machine(MachineCli::Ls) => Request::MachineList(AS_CALLER),
            Self::Machine(MachineCli::Status(a)) => Request::MachineStatus(MachineStatusReq {
                identity: None,
                name: a.name,
            }),
        })
    }
}

impl WebhookCli {
    /// Convert the parsed verb into an action.
    fn into_action(self) -> Action {
        Action::Request(match self {
            Self::Add(a) => Request::WebhookAdd(WebhookAddReq {
                identity: None,
                url: a.url,
                events: (!a.events.is_empty()).then_some(a.events),
                header: a.header,
            }),
            Self::Setup(a) => Request::WebhookSetupShow(WebhookSetupShowReq {
                identity: None,
                id: a.id,
            }),
            Self::Finish(a) => return Action::Stdin(StdinRequest::WebhookFinish(a.id)),
            Self::Ls => Request::WebhookList(AS_CALLER),
            Self::Rm(a) => Request::WebhookRemove(WebhookRemoveReq {
                identity: None,
                id: a.id,
            }),
        })
    }
}
