//! Validated names and opaque identifiers shared across moor.

/// A name or path that failed moor's validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// The Box name is not `<repo>/<label>` (or a bare segment where allowed).
    #[error(
        "invalid task name `{0}`: use <repo>/<label>, each part 1-40 lowercase letters, digits or '-', not starting with '-'"
    )]
    TaskName(String),
    /// The sha is not 4-40 hex chars.
    #[error("invalid sha `{0}`: use 4-40 hex characters")]
    Sha(String),
    /// The box workdir is not an absolute path.
    #[error("invalid box workdir `{0}`: must be an absolute path")]
    Workdir(String),
    /// The path inside the box is absolute, empty or contains `..`.
    #[error("invalid box path `{0}`: must be relative, non-empty and free of `..`")]
    RelPath(String),
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

/// Old name of [`BoxName`], kept while callers move over.
pub type TaskName = BoxName;

impl BoxName {
    /// Validates and wraps a strict Box name, `<repo>/<label>`.
    ///
    /// Exactly one `/`; each part is 1-40 chars of `[a-z0-9-]` not starting
    /// with `-`, and the repo part is also a valid repo name.
    ///
    /// # Errors
    /// Returns [`NameError::TaskName`] when `s` breaks the name rule.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        match s.split_once('/') {
            Some((repo, label))
                if valid_segment(repo) && valid_segment(label) && valid_repo_name(repo) =>
            {
                Ok(Self(s.to_owned()))
            }
            _ => Err(NameError::TaskName(s.to_owned())),
        }
    }

    /// Like [`BoxName::parse`], but also accepts a bare segment (no `/`, 1-40
    /// chars of `[a-z0-9-]`, not starting with `-`).
    ///
    /// Bare names are accepted only until the store's next schema, because the
    /// current store and Slack's `slack-<ts>` names still produce them. A string
    /// containing `/` must pass the strict rule; it is never retried as bare.
    ///
    /// # Errors
    /// Returns [`NameError::TaskName`] when `s` is neither a strict name nor a
    /// bare segment.
    pub fn parse_allow_bare(s: &str) -> Result<Self, NameError> {
        if s.contains('/') {
            Self::parse(s)
        } else if valid_segment(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::TaskName(s.to_owned()))
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

/// Names the next Box of `repo`: `<repo>/<max(used)+1>`, starting at 1.
///
/// The caller passes every n ever handed out for the repo (the high-water
/// mark), including closed Boxes, so an n is never reused. Its storage arrives
/// with the store's next schema.
///
/// # Errors
/// Returns [`NameError::TaskName`] when the result is not a valid name.
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

/// A store-assigned Box id; the Box's key. Ordering is promotion order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct BoxId(i64);

/// Old name of [`BoxId`], kept while callers move over.
pub type TaskId = BoxId;

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

/// A store-assigned Session id: the row id of one agent session in a Box.
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

    /// Returns the id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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

#[cfg(test)]
mod tests;
