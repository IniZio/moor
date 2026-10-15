//! Minimum crabbox version: moor relies on flags older builds lack.

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
    /// Build number after `-moor.`; zero when the release has no such suffix.
    moor: u64,
}

/// Oldest crabbox moor works with.
pub(super) const REQUIRED: Version = Version {
    major: 0,
    minor: 72,
    patch: 0,
    moor: 7,
};

impl Version {
    /// Reads the version from `crabbox --version` output, such as `crabbox 0.72.0-moor.7`.
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
        let moor = match build {
            Some(n) => n.parse().ok()?,
            None => 0,
        };
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
        if self.moor > 0 {
            write!(f, "-moor.{}", self.moor)?;
        }
        Ok(())
    }
}

/// Accepts `--version` output that names a release at least [`REQUIRED`].
pub(super) fn check(output: &str) -> Result<(), DriverError> {
    let found = output.trim();
    match Version::parse(output) {
        Some(v) if v >= REQUIRED => Ok(()),
        parsed => {
            let found = parsed.map_or_else(|| format!("{found:?} (unreadable)"), |v| v.to_string());
            Err(DriverError::Failed {
                summary: format!(
                    "crabbox {REQUIRED} or newer is required, found {found}; upgrade crabbox"
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests;
