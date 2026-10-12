//! Validated names and opaque identifiers shared across moor.

/// A name or path that failed moor's validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// The task name is not 1-40 chars of `[a-z0-9-]` starting with `[a-z0-9]`.
    #[error(
        "invalid task name `{0}`: use 1-40 lowercase letters, digits or '-', not starting with '-'"
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

/// A task name, `[a-z0-9][a-z0-9-]{0,39}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskName(String);

impl TaskName {
    /// Validates and wraps a task name.
    ///
    /// # Errors
    /// Returns [`NameError::TaskName`] when `s` breaks the name rule.
    pub fn parse(s: &str) -> Result<Self, NameError> {
        let mut chars = s.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if first_ok && rest_ok && s.len() <= 40 {
            Ok(Self(s.to_owned()))
        } else {
            Err(NameError::TaskName(s.to_owned()))
        }
    }

    /// Returns the name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl serde::Serialize for TaskName {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for TaskName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A store-assigned task id; ordering is promotion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(i64);

impl TaskId {
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

/// An opaque Box handle; only the Driver adapter interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
