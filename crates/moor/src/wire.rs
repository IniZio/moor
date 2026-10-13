//! Composition root: the only module that names concrete adapters.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::surface::Surface;
use moor_core::ErrorKind;

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::adapters::herdr::Herdr;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::config_load::load_user;
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
    let config = load_user(env)?;
    let cwd = std::env::current_dir()
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("current directory: {e}")))?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let extra_env = HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect();
    let surface: Arc<dyn Surface> = Arc::new(
        Herdr::new(
            Arc::clone(&proc),
            config.herdr_session().to_owned(),
            extra_env,
        )
        .seq_from(clock.now()),
    );
    let driver: Arc<dyn Driver> = Arc::new(Crabbox::new(Arc::clone(&proc)));
    let engine = BoxAgent::new(Arc::clone(&driver), Arc::clone(&surface), Arc::new(Claude));
    Ok(Cx::new(CxParts {
        state_dir: env.state_dir(),
        clock: Arc::new(clock),
        rng: Arc::new(SystemRng),
        driver,
        surface,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        git: Git::new(Arc::clone(&proc)),
        config,
        creds: Creds::new(env.clone(), proc),
        cwd,
        prompt_window: prompt_window(env),
    }))
}

/// The environment variable that sets the first-run prompt watch window, in milliseconds.
const PROMPT_WINDOW_ENV: &str = "MOOR_AGENT_PROMPT_WINDOW_MS";

/// Window used when the variable is unset or not a number.
const DEFAULT_PROMPT_WINDOW_MS: u64 = 8_000;

/// How long to watch a fresh agent pane for first-run prompts.
fn prompt_window(env: &Env) -> Duration {
    let ms = env
        .get(PROMPT_WINDOW_ENV)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_PROMPT_WINDOW_MS);
    Duration::from_millis(ms)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn window(value: Option<&str>) -> Duration {
        let pairs: Vec<(&str, &str)> = value.map(|v| (PROMPT_WINDOW_ENV, v)).into_iter().collect();
        prompt_window(&Env::from_pairs(pairs))
    }

    #[test]
    fn the_prompt_window_defaults_to_eight_seconds() {
        assert_eq!(window(None), Duration::from_secs(8));
        assert_eq!(window(Some("soon")), Duration::from_secs(8));
    }

    #[test]
    fn the_prompt_window_reads_milliseconds_from_the_environment() {
        assert_eq!(window(Some("250")), Duration::from_millis(250));
        assert_eq!(window(Some(" 0 ")), Duration::ZERO);
    }
}
