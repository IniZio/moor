//! clap definitions for `moor user`: the caller's user, its secrets and its link codes.

use clap::{Args, Subcommand};
use moor_core::request::{AsReq, LinkRedeemReq, SecretDeleteReq};
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
            Self::LinkCode => Request::LinkCode(AS_CALLER),
            Self::Link(a) => Request::LinkRedeem(LinkRedeemReq {
                identity: None,
                code: a.code,
            }),
        })
    }
}
