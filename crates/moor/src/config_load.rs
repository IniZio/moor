//! Locating and reading the user config file.

use std::path::PathBuf;

use moor_core::config::{parse_user, UserConfig};
use moor_core::ErrorKind;

use crate::env::Env;
use crate::error::CommandError;

/// User config path: `$MOOR_CONFIG`, else XDG, else `$HOME/.config`.
fn user_path(env: &Env) -> Option<PathBuf> {
    if let Some(p) = env.get("MOOR_CONFIG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    env.xdg("XDG_CONFIG_HOME", ".config", "moor/config.yaml")
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

#[cfg(test)]
mod tests;
