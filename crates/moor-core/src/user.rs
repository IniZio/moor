//! Users and the identities that name them: `<namespace>:<rest>` strings that a front or the
//! local socket asserts. Core reads nothing past the first `:`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Most bytes in an identity.
const IDENTITY_MAX: usize = 256;
/// Most bytes in a namespace or a secret name.
const NAME_MAX: usize = 64;

/// A moor user's id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct UserId(i64);

impl UserId {
    /// Wraps a raw id.
    #[must_use]
    pub const fn new(v: i64) -> Self {
        Self(v)
    }

    /// Returns the raw id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// True for `[a-z][a-z0-9-]*` of at most `max` bytes.
fn lower_word(s: &str, max: usize, more: impl Fn(u8) -> bool) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && s.len() <= max
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || more(b))
}

/// The namespace of the identities the controller's own socket asserts.
pub const LOCAL: &str = "local";

/// True when `s` can name a front's namespace: `[a-z][a-z0-9-]*` and not [`LOCAL`].
#[must_use]
pub fn valid_front_namespace(s: &str) -> bool {
    s != LOCAL && lower_word(s, NAME_MAX, |_| false)
}

/// A namespaced identity, `<namespace>:<rest>`. The namespace is `[a-z][a-z0-9-]*`; the rest
/// is opaque, non-empty, and the whole is at most 256 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Identity(String);

impl Identity {
    /// Parses an identity.
    ///
    /// # Errors
    /// A sentence naming the rule `s` breaks.
    pub fn parse(s: &str) -> Result<Self, String> {
        let Some((namespace, rest)) = s.split_once(':') else {
            return Err(format!("identity `{s}` has no `<namespace>:`"));
        };
        if !lower_word(namespace, NAME_MAX, |_| false) {
            return Err(format!(
                "identity namespace `{namespace}` must match [a-z][a-z0-9-]*"
            ));
        }
        if rest.is_empty() || s.len() > IDENTITY_MAX {
            return Err(format!(
                "identity `{namespace}:` needs 1 to {IDENTITY_MAX} bytes in all"
            ));
        }
        Ok(Self(s.to_owned()))
    }

    /// The identity of local user `uid`, as the controller's own socket sees it.
    #[must_use]
    pub fn local(uid: u32) -> Self {
        Self(format!("{LOCAL}:{uid}"))
    }

    /// The namespace, before the first `:`.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.split_once(':').map_or("", |(ns, _)| ns)
    }

    /// The whole identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Identity {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<Identity> for String {
    fn from(id: Identity) -> Self {
        id.0
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whom a controller connection speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The controller's own socket, from a peer with the controller's own uid: the local
    /// operator.
    Operator(u32),
    /// The controller's own socket, from a peer with another uid.
    Local(u32),
    /// A front's socket, bound to that front's namespace. Never the operator.
    Front(String),
}

impl Caller {
    /// The identity a request on this connection acts as. The controller's own socket acts as
    /// `local:<uid>` and may assert only that; a front socket must assert an identity in its
    /// own namespace.
    ///
    /// # Errors
    /// The refusal for an identity this connection may not assert.
    pub fn resolve(&self, asserted: Option<&Identity>) -> Result<Identity, String> {
        match (self, asserted) {
            (Self::Operator(uid) | Self::Local(uid), None) => Ok(Identity::local(*uid)),
            (Self::Operator(uid) | Self::Local(uid), Some(id)) if *id == Identity::local(*uid) => {
                Ok(id.clone())
            }
            (Self::Operator(uid) | Self::Local(uid), Some(id)) => Err(format!(
                "the controller socket speaks only for {LOCAL}:{uid}, not {id}"
            )),
            (Self::Front(ns), Some(id)) if id.namespace() == ns => Ok(id.clone()),
            (Self::Front(ns), Some(id)) => Err(format!(
                "the {ns} front socket may assert only {ns}: identities, not {id}"
            )),
            (Self::Front(ns), None) => {
                Err(format!("the {ns} front socket must assert an identity"))
            }
        }
    }
}

/// Whose Boxes a caller may act on and see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The local operator: every Box.
    Operator,
    /// One user: the Boxes that act for it.
    User(UserId),
    /// An identity no user holds: no Box.
    Nobody,
}

impl Viewer {
    /// Whether this viewer may act on a Box that acts for `owner`. A Box with no user is the
    /// operator's alone.
    #[must_use]
    pub fn may_act_on(self, owner: Option<UserId>) -> bool {
        match self {
            Self::Operator => true,
            Self::User(user) => owner == Some(user),
            Self::Nobody => false,
        }
    }
}

/// The name of one of a user's secrets: `[a-z][a-z0-9_-]*`, at most 64 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecretName(String);

impl SecretName {
    /// Parses a secret name.
    ///
    /// # Errors
    /// A sentence naming the rule `s` breaks.
    pub fn parse(s: &str) -> Result<Self, String> {
        if lower_word(s, NAME_MAX, |b| b == b'_') {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!(
                "secret name `{s}` must match [a-z][a-z0-9_-]* in at most {NAME_MAX} bytes"
            ))
        }
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SecretName {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<SecretName> for String {
    fn from(name: SecretName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests;
