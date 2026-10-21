//! Requests about moor users. Each may assert the identity it acts as; the controller checks
//! it against the socket it arrived on.

use serde::{Deserialize, Serialize};

use super::Request;
use crate::user::Identity;

/// Arguments of a user request that needs only the identity it acts as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// A user request with its asserted identity taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCmd {
    /// The user and identities of the caller.
    Whoami,
}

impl UserCmd {
    /// The command name.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Whoami => "whoami",
        }
    }
}

impl Request {
    /// Splits a user request into the identity it asserts and its command; any other request
    /// comes back unchanged.
    ///
    /// # Errors
    /// The request itself, when it is not a user request.
    pub fn into_user(self) -> Result<(Option<Identity>, UserCmd), Box<Self>> {
        match self {
            Self::Whoami(r) => Ok((r.identity, UserCmd::Whoami)),
            other => Err(Box::new(other)),
        }
    }
}

#[cfg(test)]
mod tests;
