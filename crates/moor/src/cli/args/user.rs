//! clap definitions for `moor user`: the caller's user, its secrets and its link codes.

use clap::{Args, Subcommand};
use moor_core::request::{AsReq, LinkRedeemReq, McpAddReq, McpRemoveReq, SecretDeleteReq};
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
    /// Mint a one-time code that links another identity to your user.
    LinkCode,
    /// Link your identity to the user that minted CODE.
    Link(LinkArgs),
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
    /// Add or replace server NAME at URL; set its key with `moor user secret set mcp_<name>`.
    Add(McpAddArgs),
    /// Remove server NAME and its key.
    Rm(McpNameArg),
    /// List your servers.
    Ls,
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

/// Arguments of `user link`.
#[derive(Debug, Args)]
pub(super) struct LinkArgs {
    /// The code `moor user link-code` printed for the other identity.
    code: String,
}

/// The `local:<uid>` identity is the socket's own, so no request asserts one.
const AS_CALLER: AsReq = AsReq { identity: None };

impl UserCli {
    /// Convert the parsed verb into an action.
    pub(super) fn into_action(self) -> Action {
        Action::Request(match self {
            Self::Enroll => Request::Enroll(AS_CALLER),
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
            Self::LinkCode => Request::LinkCode(AS_CALLER),
            Self::Link(a) => Request::LinkRedeem(LinkRedeemReq {
                identity: None,
                code: a.code,
            }),
        })
    }
}
