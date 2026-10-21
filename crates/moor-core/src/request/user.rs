//! Requests about moor users. Each may assert the identity it acts as; the controller checks
//! it against the socket it arrived on.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Request;
use crate::secret::Secret;
use crate::user::{Identity, SecretName};

/// Arguments of a user request that needs only the identity it acts as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// Arguments of `secret-set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSetReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
    /// The secret's value.
    pub value: SecretValue,
}

/// Arguments of `secret-delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretDeleteReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The secret's name.
    pub name: SecretName,
}

/// Arguments of `link-redeem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRedeemReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The code `link-code` minted.
    pub code: String,
}

/// Arguments of `start-needs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartNeedsReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// Repo known to the controller, by name or url, as `start` takes it; the invocation
    /// directory's repo when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// A secret value in a request. Its JSON form is the text itself, because `secret-set` must
/// carry it over the socket; `Debug` never shows it. Serialize a request only to send it.
pub struct SecretValue(Secret);

impl SecretValue {
    /// Wraps the value's bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Secret::new(bytes))
    }

    /// The value.
    #[must_use]
    pub const fn secret(&self) -> &Secret {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self::new(self.0.expose().to_vec())
    }
}

impl PartialEq for SecretValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose() == other.0.expose()
    }
}

impl Eq for SecretValue {}

impl Serialize for SecretValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let text = std::str::from_utf8(self.0.expose())
            .map_err(|_| serde::ser::Error::custom("a secret value must be UTF-8 text"))?;
        s.serialize_str(text)
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|text| Self::new(text.into_bytes()))
    }
}

/// A user request with its asserted identity taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCmd {
    /// The user and identities of the caller.
    Whoami,
    /// Create a user for the caller, or return the one it has.
    Enroll,
    /// Store one of the caller's user's secrets.
    SecretSet {
        /// The secret's name.
        name: SecretName,
        /// The secret's value.
        value: SecretValue,
    },
    /// Delete one of the caller's user's secrets.
    SecretDelete {
        /// The secret's name.
        name: SecretName,
    },
    /// The names of the caller's user's secrets.
    SecretNames,
    /// Mint a one-time code that links another identity to the caller's user.
    LinkCode,
    /// Link the caller to the user that minted `code`.
    LinkRedeem {
        /// The code.
        code: String,
    },
}

impl UserCmd {
    /// The command name.
    #[must_use]
    pub const fn cmd(&self) -> &'static str {
        match self {
            Self::Whoami => "whoami",
            Self::Enroll => "enroll",
            Self::SecretSet { .. } => "secret-set",
            Self::SecretDelete { .. } => "secret-delete",
            Self::SecretNames => "secret-names",
            Self::LinkCode => "link-code",
            Self::LinkRedeem { .. } => "link-redeem",
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
        Ok(match self {
            Self::Whoami(r) => (r.identity, UserCmd::Whoami),
            Self::Enroll(r) => (r.identity, UserCmd::Enroll),
            Self::SecretSet(r) => (
                r.identity,
                UserCmd::SecretSet {
                    name: r.name,
                    value: r.value,
                },
            ),
            Self::SecretDelete(r) => (r.identity, UserCmd::SecretDelete { name: r.name }),
            Self::SecretNames(r) => (r.identity, UserCmd::SecretNames),
            Self::LinkCode(r) => (r.identity, UserCmd::LinkCode),
            Self::LinkRedeem(r) => (r.identity, UserCmd::LinkRedeem { code: r.code }),
            other => return Err(Box::new(other)),
        })
    }
}

#[cfg(test)]
mod tests;
