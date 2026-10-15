//! Locating and reading the user and repo config files.

use std::path::{Path, PathBuf};

use moor_core::config::{parse_repo, parse_user, RepoConfig, UserConfig};
use moor_core::ErrorKind;

use crate::env::Env;
use crate::error::CommandError;

/// File name of the repo config.
const REPO_FILE: &str = ".moor.yaml";

/// User config path: `$MOOR_CONFIG`, else XDG, else `$HOME/.config`.
fn user_path(env: &Env) -> Option<PathBuf> {
    if let Some(p) = env.get("MOOR_CONFIG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    env.xdg("XDG_CONFIG_HOME", ".config", "moor/config.yaml")
}

/// The moor controller's own state directory and config directory, which a Box on this host
/// must not reach. Empty when neither location is known.
pub(crate) fn private_paths(env: &Env) -> Vec<String> {
    let config = user_path(env).and_then(|p| p.parent().map(Path::to_path_buf));
    [env.state_dir(), config]
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Read the user config; no known config location or a missing file means defaults.
pub(crate) fn load_user(env: &Env) -> Result<UserConfig, CommandError> {
    let Some(path) = user_path(env) else {
        return Ok(UserConfig::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(parse_user(&path.display().to_string(), &text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserConfig::default()),
        Err(e) => Err(CommandError::new(
            ErrorKind::Failed,
            format!("read {}: {e}", path.display()),
        )),
    }
}

/// Read the repo config: the worktree's own file, else the main checkout's.
pub(crate) fn load_repo(
    worktree: &Path,
    main_checkout: &Path,
) -> Result<Option<RepoConfig>, CommandError> {
    for dir in [worktree, main_checkout] {
        let path = dir.join(REPO_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => return Ok(Some(parse_repo(&path.display().to_string(), &text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CommandError::new(
                    ErrorKind::Failed,
                    format!("read {}: {e}", path.display()),
                ))
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
