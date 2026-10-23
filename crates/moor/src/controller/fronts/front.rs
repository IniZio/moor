//! A front the controller runs, and the paths it may reach through the covers.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use moor_core::config::FrontEntry;

#[cfg(target_os = "linux")]
use crate::controller::front_shim::refuse_under_tmp;
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
        let resolv = fs::canonicalize("/etc/resolv.conf").ok();
        Some(
            refuse_read_only(ns, &entry.read_only, env, operator, resolv.as_deref()).and_then(
                |()| {
                    let socket = listener
                        .local_addr()
                        .ok()
                        .and_then(|a| a.as_pathname().map(PathBuf::from))
                        .ok_or_else(|| format!("front {ns}: its socket has no path"))?;
                    let mut read_only = entry.read_only.clone();
                    let path = std::env::var_os("PATH").unwrap_or_default();
                    if let Some(word) = command.first_mut() {
                        if let Some(found) = program_path(ns, word, &path)? {
                            *word = found.display().to_string();
                            read_only.push(found);
                        }
                    }
                    Ok(Self {
                        ns: ns.to_owned(),
                        socket,
                        command,
                        state_dir: entry.state_dir.clone(),
                        read_only,
                    })
                },
            ),
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
    resolv: Option<&Path>,
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
    #[cfg(target_os = "linux")]
    if let Some(real) = resolv {
        refuse_under_tmp(&format!("fronts.{ns}: /etc/resolv.conf resolves to"), real)?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = resolv;
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

/// The real path of front `ns`'s `program`, none when it cannot be found; an error when that
/// path is or lies under `/tmp`, which the front's own tmp dir would hide on Linux.
#[cfg_attr(
    not(target_os = "linux"),
    expect(
        clippy::unnecessary_wraps,
        reason = "only Linux gives the front a /tmp of its own to refuse"
    )
)]
fn program_path(ns: &str, program: &str, path: &OsStr) -> Result<Option<PathBuf>, String> {
    let found = resolve(program, path);
    #[cfg(target_os = "linux")]
    if let Some(found) = &found {
        refuse_under_tmp(&format!("fronts.{ns}: its program"), found)?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = ns;
    Ok(found)
}

/// The real path of `program`: itself when it names a path, else the first match on `path`;
/// none when it cannot be found.
fn resolve(program: &str, path: &OsStr) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).canonicalize().ok();
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(OsStr::new(program)))
        .find(|candidate| candidate.is_file())
        .and_then(|found| found.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;

    use super::{program_path, refuse_read_only};
    use crate::env::Env;

    #[test]
    fn a_controller_state_dir_in_a_system_dir_starts_no_front() {
        let env = Env::from_pairs([("MOOR_STATE_DIR", "/etc/moor-front-test")]);
        let e =
            refuse_read_only("web", &[], &env, Path::new("/nonexistent/c.sock"), None).unwrap_err();
        assert!(e.contains("/etc/moor-front-test lies in /etc"), "{e}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_resolv_conf_under_tmp_starts_no_front() {
        let env = Env::from_pairs([("MOOR_STATE_DIR", "/var/moor-front-test")]);
        let operator = Path::new("/nonexistent/c.sock");
        let under = Some(Path::new("/tmp/run/resolv.conf"));
        let e = refuse_read_only("web", &[], &env, operator, under).unwrap_err();
        assert!(
            e.contains("/etc/resolv.conf resolves to /tmp/run/resolv.conf"),
            "{e}"
        );
        assert!(e.contains("front's own tmp dir"), "{e}");
        let elsewhere = Some(Path::new("/run/resolv.conf"));
        assert_eq!(
            refuse_read_only("web", &[], &env, operator, elsewhere),
            Ok(())
        );
    }

    #[cfg(target_os = "linux")]
    fn program_under_tmp() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let real = dir.path().canonicalize().unwrap().join("front-bin");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, real)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_program_under_tmp_found_on_path_starts_no_front() {
        let (dir, real) = program_under_tmp();
        let e = program_path("web", "front-bin", dir.path().as_os_str()).unwrap_err();
        assert!(e.starts_with("fronts.web: its program "), "{e}");
        assert!(e.contains(&real.display().to_string()), "{e}");
        assert!(e.contains("front's own tmp dir"), "{e}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_program_under_tmp_named_by_path_starts_no_front() {
        let (_dir, real) = program_under_tmp();
        let e = program_path("web", real.to_str().unwrap(), "".as_ref()).unwrap_err();
        assert!(e.contains("its program"), "{e}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn new_refuses_a_front_whose_program_lies_under_tmp() {
        let (dir, real) = program_under_tmp();
        let listener = std::os::unix::net::UnixListener::bind(dir.path().join("s")).unwrap();
        let entry = moor_core::config::FrontEntry {
            command: Some(vec![real.display().to_string()]),
            ..Default::default()
        };
        let env = Env::from_pairs([("MOOR_STATE_DIR", "/var/moor-front-test")]);
        let operator = Path::new("/nonexistent/c.sock");
        let made = super::Front::new("web", &listener, &entry, &env, operator).unwrap();
        let e = made.err().unwrap();
        assert!(e.starts_with("fronts.web: its program "), "{e}");
    }

    #[test]
    fn a_program_outside_tmp_or_not_found_is_accepted() {
        assert_eq!(
            program_path("web", "no-such-front-bin", "/bin".as_ref()),
            Ok(None)
        );
        let found = program_path("web", "sh", "/bin:/usr/bin".as_ref()).unwrap();
        assert!(found.is_some());
    }
}
