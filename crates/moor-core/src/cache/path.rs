//! The Box paths a cache volume may cover.

use super::CacheError;
use crate::box_key::{HOME_PREFIX, WORK_ROOT};
use crate::exec_env::per_box_paths;

/// Roots a cache path may start with: the login home, and two system trees moor never writes.
const ROOTS: [&str; 3] = ["~/", "/opt/", "/var/cache/"];
/// The login user's ssh directory; the provider writes the Box's keys there.
const SSH_DIR: &str = "~/.ssh";

/// A directory in the Box: `~/<rel>` (the login home), `/opt/<rel>` or `/var/cache/<rel>`.
/// Never equal to, above or below a path in [`owned_paths`], nor a `~/.moor-*` directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CachePath(String);

impl CachePath {
    /// Validates `s`; a trailing `/` is dropped.
    ///
    /// # Errors
    /// [`CacheError::Path`] for another root, an empty, `.` or `..` segment, a character outside
    /// letters, digits and `._-+@`, or a path that meets one moor writes for each Box.
    pub fn parse(s: &str) -> Result<Self, CacheError> {
        let bad = |why: String| CacheError::Path {
            path: s.to_owned(),
            why,
        };
        let Some((root, rel)) = ROOTS.iter().find_map(|r| Some((*r, s.strip_prefix(r)?))) else {
            return Err(bad("must start with ~/, /opt/ or /var/cache/".to_owned()));
        };
        let rel = rel.strip_suffix('/').unwrap_or(rel);
        if rel.is_empty()
            || rel
                .split('/')
                .any(|g| g.is_empty() || g == "." || g == "..")
        {
            return Err(bad(
                "must name a directory below its root, with no empty, . or .. segment".to_owned(),
            ));
        }
        if !rel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+@/".contains(&b))
        {
            return Err(bad(
                "may hold only letters, digits, / and . _ - + @".to_owned()
            ));
        }
        let path = Self(format!("{root}{rel}"));
        let owned = owned_paths()
            .into_iter()
            .find(|o| path.overlaps(&Self(o.clone())));
        if let Some(o) = owned {
            return Err(bad(format!(
                "moor writes {o} in every Box, so a cache must not be, hold or sit inside it"
            )));
        }
        if root == "~/" && rel.starts_with(HOME_PREFIX) {
            return Err(bad(format!(
                "~/{HOME_PREFIX}* directories belong to moor in every Box"
            )));
        }
        Ok(path)
    }

    /// The path as written, like `~/.gradle/caches`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether one path is the other or lies below it.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        let (a, b) = (self.0.split('/'), other.0.split('/'));
        a.zip(b).all(|(x, y)| x == y)
    }
}

/// Paths moor itself writes in every Box, in `~/` form: what each exec rewrites (the Proxy CA
/// directory, Gradle's `gradle.properties` and trust store), the workdir root and `~/.ssh`.
#[must_use]
pub fn owned_paths() -> Vec<String> {
    let per_box = per_box_paths()
        .into_iter()
        .map(|p| p.replacen("$HOME/", "~/", 1));
    per_box
        .chain([format!("~/{WORK_ROOT}"), SSH_DIR.to_owned()])
        .collect()
}
