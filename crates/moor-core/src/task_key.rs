//! The single key every macOS Box path and cleanup match is derived from.

use crate::name::{BoxPath, NameError, TaskName};

/// The one key every Mac Box path and cleanup match is derived from: the Task name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskKey(String);

impl TaskKey {
    /// Derives the key from a Task name.
    #[must_use]
    pub fn new(name: &TaskName) -> Self {
        Self(name.as_str().to_owned())
    }

    /// Returns the key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every directory of one Task's macOS Box, all derived from one [`TaskKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacLayout {
    /// `<work_root>/<key>`.
    pub root: BoxPath,
    /// The checkout.
    pub work: BoxPath,
    /// The sandbox `HOME`.
    pub home: BoxPath,
    /// The sandbox `TMPDIR`.
    pub tmp: BoxPath,
    /// Build and tool caches.
    pub cache: BoxPath,
    /// Simulator data.
    pub sim: BoxPath,
    /// moor-written files, read-only to the sandbox.
    pub launch: BoxPath,
}

/// Rejects text that is not one plain path component.
fn component(s: &str) -> Result<&str, NameError> {
    if s.is_empty() || s == "." || s == ".." || s.contains('/') {
        return Err(NameError::RelPath(s.to_owned()));
    }
    Ok(s)
}

impl MacLayout {
    /// `<work_root>/<key>/{work,home,tmp,cache,sim,launch}`.
    ///
    /// # Errors
    /// Returns a [`NameError`] when the key is not one plain path component or
    /// `work_root` is not an absolute path.
    pub fn new(work_root: &BoxPath, key: &TaskKey) -> Result<Self, NameError> {
        let root = BoxPath::under(work_root.as_str(), component(key.as_str())?)?;
        let sub = |name: &str| BoxPath::under(root.as_str(), name);
        Ok(Self {
            work: sub("work")?,
            home: sub("home")?,
            tmp: sub("tmp")?,
            cache: sub("cache")?,
            sim: sub("sim")?,
            launch: sub("launch")?,
            root,
        })
    }

    /// The layout whose `work/` is `workdir` or an ancestor of it, for this key.
    ///
    /// Matches whole path components only.
    #[must_use]
    pub fn find(workdir: &BoxPath, key: &TaskKey) -> Option<Self> {
        let text = workdir.as_str();
        let marker = format!("/{}/work", key.as_str());
        let at = text.match_indices(&marker).find_map(|(i, m)| {
            let rest = text.get(i + m.len()..)?;
            (rest.is_empty() || rest.starts_with('/')).then_some(i)
        })?;
        let parent = text.get(..at).filter(|p| !p.is_empty()).unwrap_or("/");
        Self::new(&BoxPath::restore(parent).ok()?, key).ok()
    }

    /// `launch/.moor-cred-<nonce>`.
    ///
    /// # Errors
    /// Returns [`NameError::RelPath`] when `nonce` is not a plain component.
    pub fn cred_dir(&self, nonce: &str) -> Result<BoxPath, NameError> {
        BoxPath::under(
            self.launch.as_str(),
            &format!(".moor-cred-{}", component(nonce)?),
        )
    }

    /// `tmp/.moor-tmp-<nonce>`.
    ///
    /// # Errors
    /// Returns [`NameError::RelPath`] when `nonce` is not a plain component.
    pub fn cred_tmp(&self, nonce: &str) -> Result<BoxPath, NameError> {
        BoxPath::under(
            self.tmp.as_str(),
            &format!(".moor-tmp-{}", component(nonce)?),
        )
    }
}

/// Login-home directories that hold credentials, relative to the login home.
/// They are never granted to a Task or linked into its home.
pub const CREDENTIAL_DIRS: [&str; 3] = [".config/gh", ".docker", ".ssh"];

/// True when `path` is `home` or below it (component-wise).
#[must_use]
pub fn inside(path: &BoxPath, home: &BoxPath) -> bool {
    let (p, h) = (
        path.as_str().trim_end_matches('/'),
        home.as_str().trim_end_matches('/'),
    );
    p == h || p.strip_prefix(h).is_some_and(|rest| rest.starts_with('/'))
}

/// Refuses a macOS Box whose workdir is inside the login home.
///
/// # Errors
/// Returns a message naming both paths when `workdir` is inside `login_home`.
pub fn refuse_inside_login_home(workdir: &BoxPath, login_home: &BoxPath) -> Result<(), String> {
    if inside(workdir, login_home) {
        return Err(format!(
            "workdir {} is inside the login home {}; a macOS Box workdir must be outside the login home",
            workdir.as_str(),
            login_home.as_str()
        ));
    }
    Ok(())
}

mod launch_files;
pub mod release;
pub mod release_sim;

#[cfg(test)]
mod tests;
