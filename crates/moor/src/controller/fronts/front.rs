//! A front the controller runs, and the paths it may reach through the covers.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use moor_core::config::FrontEntry;

use crate::controller::front_shim::SYSTEM;
use crate::env::Env;

/// A front the controller runs.
pub(in crate::controller) struct Front {
    /// The front's namespace.
    pub(super) ns: String,
    /// Its socket; the child gets an `O_PATH` fd to it.
    pub(super) socket: PathBuf,
    /// Its argv, not empty, its program resolved to a real path when it can be found.
    pub(super) command: Vec<String>,
    /// The one directory it may write.
    pub(super) state_dir: Option<PathBuf>,
    /// Paths it may read even where a cover hides them: its `read_only` entries and its
    /// program.
    pub(super) read_only: Vec<PathBuf>,
}

impl Front {
    /// The front `ns` listening on `listener`, run as `entry` says; none when `entry` has no
    /// command. Refuses a `read_only` path that would show the front the controller's state
    /// dir, config dir, operator socket or vault key.
    pub(in crate::controller) fn new(
        ns: &str,
        listener: &UnixListener,
        entry: &FrontEntry,
        env: &Env,
        operator: &Path,
    ) -> Option<Result<Self, String>> {
        let mut command = entry.command.clone()?;
        Some(
            refuse_read_only(ns, &entry.read_only, env, operator).and_then(|()| {
                let socket = listener
                    .local_addr()
                    .ok()
                    .and_then(|a| a.as_pathname().map(PathBuf::from))
                    .ok_or_else(|| format!("front {ns}: its socket has no path"))?;
                let mut read_only = entry.read_only.clone();
                let program = command.first_mut().map(|p| (resolve(p), p));
                if let Some((Some(found), word)) = program {
                    *word = found.display().to_string();
                    read_only.push(found);
                }
                Ok(Self {
                    ns: ns.to_owned(),
                    socket,
                    command,
                    state_dir: entry.state_dir.clone(),
                    read_only,
                })
            }),
        )
    }

    /// `<state>/fronts/<ns>.pid`: the running front's pid and its start time from `/proc`.
    pub(super) fn pid_file(&self) -> PathBuf {
        self.socket.with_extension("pid")
    }
}

/// Refuses any of `read_only` that is not a real path (a symlink or `..` in it) or that equals,
/// holds or lies inside the controller's state dir, config dir, vault key or `operator` socket;
/// and refuses those controller paths when a system dir every front reads holds them.
fn refuse_read_only(
    ns: &str,
    read_only: &[PathBuf],
    env: &Env,
    operator: &Path,
) -> Result<(), String> {
    let config_dir = env.vault_key().and_then(Path::parent).map(Path::to_owned);
    let guarded: Vec<PathBuf> = [
        env.state_dir(),
        config_dir,
        env.vault_key().map(Path::to_owned),
        Some(operator.to_owned()),
    ]
    .into_iter()
    .flatten()
    .map(|path| fs::canonicalize(&path).unwrap_or(path))
    .collect();
    let system = SYSTEM.iter().filter_map(|dir| fs::canonicalize(dir).ok());
    for dir in system {
        if let Some(inside) = guarded.iter().find(|path| path.starts_with(&dir)) {
            return Err(format!(
                "fronts.{ns}: {} lies in {}, which every front can read",
                inside.display(),
                dir.display()
            ));
        }
    }
    for path in read_only {
        let real = fs::canonicalize(path)
            .map_err(|e| format!("fronts.{ns}.read_only: {}: {e}", path.display()))?;
        if real != *path {
            return Err(format!(
                "fronts.{ns}.read_only: {} is not a real path; it resolves to {}",
                path.display(),
                real.display()
            ));
        }
        if guarded
            .iter()
            .any(|guarded| guarded.starts_with(&real) || real.starts_with(guarded))
        {
            return Err(format!(
                "fronts.{ns}.read_only: {} would show the controller's state dir, config \
                 dir, vault key or socket",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The real path of `program`: itself when it names a path, else the first match on this
/// process's `PATH`; none when it cannot be found.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).canonicalize().ok();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(OsStr::new(program)))
        .find(|candidate| candidate.is_file())
        .and_then(|found| found.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::refuse_read_only;
    use crate::env::Env;

    #[test]
    fn a_controller_state_dir_in_a_system_dir_starts_no_front() {
        let env = Env::from_pairs([("MOOR_STATE_DIR", "/etc/moor-front-test")]);
        let e = refuse_read_only("web", &[], &env, Path::new("/nonexistent/c.sock")).unwrap_err();
        assert!(e.contains("/etc/moor-front-test lies in /etc"), "{e}");
    }
}
