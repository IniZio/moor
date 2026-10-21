//! Minimum crabbox version: moor relies on flags that only the IniZio/crabbox `-moor` builds have.

use moor_core::seam::driver::DriverError;

/// A crabbox release: `major.minor.patch` with an optional `-moor.N` build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Version {
    /// Major number.
    major: u64,
    /// Minor number.
    minor: u64,
    /// Patch number.
    patch: u64,
    /// Build number after `-moor.`; `None` for an upstream release.
    moor: Option<u64>,
}

/// Oldest crabbox moor works with.
pub(super) const REQUIRED: Version = Version {
    major: 0,
    minor: 79,
    patch: 1,
    moor: Some(1),
};

impl Version {
    /// Reads the version from `crabbox --version` output, such as `crabbox 0.79.1-moor.1`.
    pub(super) fn parse(output: &str) -> Option<Self> {
        let token = output
            .split_whitespace()
            .map(|t| t.strip_prefix('v').unwrap_or(t))
            .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))?;
        let (core, build) = match token.split_once('-') {
            Some((core, rest)) => (core, Some(rest.strip_prefix("moor.")?)),
            None => (token, None),
        };
        let mut parts = core.split('.');
        let mut next = || parts.next()?.parse::<u64>().ok();
        let (major, minor, patch) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return None;
        }
        let moor = build.map(str::parse).transpose().ok()?;
        Some(Self {
            major,
            minor,
            patch,
            moor,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(n) = self.moor {
            write!(f, "-moor.{n}")?;
        }
        Ok(())
    }
}

/// Accepts `--version` output that names a `-moor` build at least [`REQUIRED`]. An upstream
/// release lacks the flags whatever its number.
pub(super) fn check(output: &str) -> Result<(), DriverError> {
    let found = output.trim();
    match Version::parse(output) {
        Some(v) if v.moor.is_some() && v >= REQUIRED => Ok(()),
        parsed => {
            let found = parsed.map_or_else(|| format!("{found:?} (unreadable)"), |v| v.to_string());
            Err(DriverError::Failed {
                summary: format!(
                    "crabbox {REQUIRED} or a newer -moor build of IniZio/crabbox is required, \
                     found {found}; install it"
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests;
