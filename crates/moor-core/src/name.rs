//! Validated names and opaque identifiers shared across moor.

/// A name or path that failed moor's validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// The Box name is not `<repo>/<label>` (or a bare segment where allowed).
    #[error(
        "invalid Box name `{0}`: use <repo>/<label>, each part 1-40 lowercase letters, digits or '-', not starting with '-' and without '--'"
    )]
    BoxName(String),
    /// The sha is not 4-40 hex chars.
    #[error("invalid sha `{0}`: use 4-40 hex characters")]
    Sha(String),
    /// The box workdir is not an absolute path.
    #[error("invalid box workdir `{0}`: must be an absolute path")]
    Workdir(String),
    /// The path inside the box is absolute, empty or contains `..`.
    #[error("invalid box path `{0}`: must be relative, non-empty and free of `..`")]
    RelPath(String),
    /// The node name is not `[a-z][a-z0-9-]*` of 1-40 bytes.
    #[error("invalid node name `{0}`: use 1-40 lowercase letters, digits or '-', starting with a letter")]
    NodeName(String),
    /// The provider id has more than one `@`, or an empty side.
    #[error("invalid provider `{0}`: use <provider> or <provider>@<machine>, each side non-empty")]
    Provider(String),
}

/// A repo registry name becomes a directory name, so it must not carry path syntax.
#[must_use]
pub fn valid_repo_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A Box name: `<repo>/<label>`, or a bare segment from older stores.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoxName(String);

impl BoxName {
    /// Validates and wraps a strict Box name, `<repo>/<label>`.
    ///
    /// Exactly one `/`; each part is 1-40 chars of `[a-z0-9-]` not starting
    /// with `-` and free of `--`, and the repo part is also a valid repo name.
    ///
    /// # Errors
    /// Returns [`NameError::BoxName`] when `s` breaks the name rule.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        match s.split_once('/') {
            Some((repo, label))
                if valid_strict_segment(repo)
                    && valid_strict_segment(label)
                    && valid_repo_name(repo) =>
            {
                Ok(Self(s.to_owned()))
            }
            _ => Err(NameError::BoxName(s.to_owned())),
        }
    }

    /// Like [`BoxName::parse`], but also accepts a bare segment (no `/`, 1-40
    /// chars of `[a-z0-9-]`, not starting with `-`).
    ///
    /// Bare names are accepted only until the store's next schema, because the
    /// current store and a front's thread-derived names still produce them. A string
    /// containing `/` must pass the strict rule; it is never retried as bare.
    ///
    /// # Errors
    /// Returns [`NameError::BoxName`] when `s` is neither a strict name nor a
    /// bare segment.
    pub fn parse_allow_bare(s: &str) -> Result<Self, NameError> {
        if s.contains('/') {
            Self::parse(s)
        } else if valid_segment(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::BoxName(s.to_owned()))
        }
    }

    /// Returns the repo part, or `None` for a bare name.
    #[must_use]
    pub fn repo(&self) -> Option<&str> {
        self.0.split_once('/').map(|(repo, _)| repo)
    }

    /// Returns the label part; the whole name when it is bare.
    #[must_use]
    pub fn label(&self) -> &str {
        self.0.split_once('/').map_or(&self.0, |(_, label)| label)
    }

    /// Returns the name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// True for 1-40 chars of `[a-z0-9-]` not starting with `-`.
fn valid_segment(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && s.len() <= 40
}

/// A segment of a strict name: a valid segment without `--`, so `/` can map to `--` in a branch.
fn valid_strict_segment(s: &str) -> bool {
    valid_segment(s) && !s.contains("--")
}

/// Names the next Box of `repo`: `<repo>/<max(used)+1>`, starting at 1.
///
/// The caller passes every n ever handed out for the repo (the high-water
/// mark), including closed Boxes, so an n is never reused. Its storage arrives
/// with the store's next schema.
///
/// # Errors
/// Returns [`NameError::BoxName`] when the result is not a valid name.
pub fn next_box_name(repo: &str, used: &[u64]) -> Result<BoxName, NameError> {
    let next = used
        .iter()
        .copied()
        .max()
        .map_or(1, |m| m.saturating_add(1));
    BoxName::parse(&format!("{repo}/{next}"))
}

impl serde::Serialize for BoxName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for BoxName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse_allow_bare(&s).map_err(serde::de::Error::custom)
    }
}

/// A node's name: `[a-z][a-z0-9-]*`, at most 40 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeName(String);

impl NodeName {
    /// Validates and wraps a node name.
    ///
    /// # Errors
    /// Returns [`NameError::NodeName`] when `s` breaks the name rule.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        let mut bytes = s.bytes();
        let ok = bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && s.len() <= 40
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::NodeName(s.to_owned()))
        }
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A store-assigned Box id; the Box's key. Ordering is promotion order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct BoxId(i64);

impl BoxId {
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

/// A store-assigned run id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct RunId(i64);

impl RunId {
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

/// A store-assigned Session id: the row id of one Session, in a Box or on the host.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SessionId(i64);

impl SessionId {
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

/// An opaque Box handle; only the Driver adapter interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BoxHandle(String);

impl BoxHandle {
    /// Wraps an opaque handle.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the handle text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An absolute path inside a Box.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoxPath(String);

impl BoxPath {
    /// Joins a relative path onto an absolute workdir.
    ///
    /// # Errors
    /// Returns [`NameError::Workdir`] for a non-absolute workdir and
    /// [`NameError::RelPath`] for an empty, absolute or `..` relative path.
    pub fn under(workdir: &str, rel: &str) -> Result<Self, NameError> {
        if !workdir.starts_with('/') {
            return Err(NameError::Workdir(workdir.to_owned()));
        }
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            return Err(NameError::RelPath(rel.to_owned()));
        }
        Ok(Self(format!("{}/{rel}", workdir.trim_end_matches('/'))))
    }

    /// Rebuilds a path from text stored earlier, keeping it unchanged.
    ///
    /// The root a Driver reports is trusted as is, `..` included; `under` guards
    /// only the relative name.
    ///
    /// # Errors
    /// Returns [`NameError::Workdir`] unless `stored` is absolute and not `/`.
    pub fn restore(stored: &str) -> Result<Self, NameError> {
        if stored.len() < 2 || !stored.starts_with('/') {
            return Err(NameError::Workdir(stored.to_owned()));
        }
        Ok(Self(stored.to_owned()))
    }

    /// Returns the path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An identifier of a Driver provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderId(String);

impl ProviderId {
    /// Wraps a provider id.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Validates a provider id: `<provider>`, or `<provider>@<host>` naming a machine of the
    /// user that hosts the Box.
    ///
    /// # Errors
    /// Returns [`NameError::Provider`] for an empty id, more than one `@`, or an empty side.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        let mut parts = s.split('@');
        let ok = parts.next().is_some_and(|base| !base.is_empty())
            && parts.next().is_none_or(|host| !host.is_empty())
            && parts.next().is_none();
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::Provider(s.to_owned()))
        }
    }

    /// Returns the id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The provider before any `@`.
    #[must_use]
    pub fn base(&self) -> &str {
        self.0.split_once('@').map_or(&self.0, |(base, _)| base)
    }

    /// The machine after the `@`; none when the id names no host.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.0.split_once('@').map(|(_, host)| host)
    }
}

/// A git sha of 4-40 hex chars, stored lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha(String);

impl Sha {
    /// Validates a sha and lowercases it.
    ///
    /// # Errors
    /// Returns [`NameError::Sha`] unless `s` is 4-40 hex characters.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        if (4..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err(NameError::Sha(s.to_owned()))
        }
    }

    /// Returns the lowercase sha text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

mod forge;
pub use forge::{ForgeRepo, GitAuthor, RepoNameError};
mod profile_source;
pub use profile_source::{ProfileSource, ProfileSourceError};

#[cfg(test)]
mod tests;
