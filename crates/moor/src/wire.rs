//! Composition root: the only module that names concrete adapters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::{parse_user, UserConfig};
use moor_core::seam::proc::Proc;
use moor_core::ErrorKind;

use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::git::Git;
use crate::adapters::herdr::Herdr;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::error::CommandError;

/// Host variables herdr needs to find its socket and terminal.
const HERDR_PASSTHROUGH: [&str; 5] = [
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "TERM",
];

/// Wire real adapters from the environment into a context.
pub(crate) fn build(env: &Env) -> Result<Cx, CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_config(env)?;
    let cwd = std::env::current_dir()
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("current directory: {e}")))?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let extra_env = HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect();
    let herdr = Herdr::new(
        Arc::clone(&proc),
        config.herdr_session().to_owned(),
        extra_env,
    );
    Ok(Cx::new(CxParts {
        state_dir: env.state_dir(),
        clock: Arc::new(clock),
        rng: Arc::new(SystemRng),
        driver: Arc::new(Crabbox::new(Arc::clone(&proc))),
        surface: Arc::new(herdr),
        git: Git::new(proc),
        config,
        cwd,
    }))
}

/// Tool overrides from config.
fn tool_paths(config: &UserConfig) -> ToolPaths {
    let t = &config.tools;
    ToolPaths {
        crabbox: t.crabbox.clone(),
        herdr: t.herdr.clone(),
        claude: t.claude.clone(),
        gh: t.gh.clone(),
        ssh: t.ssh.clone(),
        git: t.git.clone(),
    }
}

/// Config file path: `$MOOR_CONFIG`, else XDG, else `$HOME/.config`.
fn config_path(env: &Env) -> Option<PathBuf> {
    let set = |k| env.get(k).filter(|v| !v.is_empty());
    if let Some(p) = set("MOOR_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = match set("XDG_CONFIG_HOME") {
        Some(x) => PathBuf::from(x),
        None => Path::new(set("HOME")?).join(".config"),
    };
    Some(base.join("moor/config.yaml"))
}

/// Read the user config; a missing file means defaults.
fn load_config(env: &Env) -> Result<UserConfig, CommandError> {
    let Some(path) = config_path(env) else {
        return Ok(UserConfig::default());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UserConfig::default()),
        Err(e) => {
            return Err(CommandError::new(
                ErrorKind::Failed,
                format!("read {}: {e}", path.display()),
            ))
        }
    };
    parse_user(&text)
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("{}: {e}", path.display())))
}
