//! Composition root: the only module that names concrete adapters.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::TaskId;
use signal_hook::consts::{SIGINT, SIGTERM};

use moor_core::config::{DriverConfig, UserConfig};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::ErrorKind;

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::nono_release::{NonoRelease, UreqFetch};
use crate::adapters::box_agent::BoxAgent;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::config_load::load_user;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::error::CommandError;
use crate::proxy::adopt;
use crate::proxy::detached::DetachedSupervisor;
use crate::proxy::nono::capture::CredsFn;
use crate::proxy::{serve, Deps, Sink};

/// Host variables herdr needs to find its socket and terminal.
const HERDR_PASSTHROUGH: [&str; 5] = [
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "TERM",
];

/// The herdr adapter, numbering its reports from `clock`.
fn herdr(
    proc: &Arc<dyn Proc>,
    session: &str,
    extra_env: Vec<(String, String)>,
    clock: Arc<dyn Clock>,
) -> Herdr {
    Herdr::new(Arc::clone(proc), session.to_owned(), extra_env).with_clock(clock)
}

/// Per-provider lookup deadlines for the providers that configure one.
fn lookup_deadlines(drivers: &BTreeMap<String, DriverConfig>) -> BTreeMap<String, Duration> {
    drivers
        .iter()
        .filter_map(|(provider, d)| {
            Some((
                provider.clone(),
                Duration::from_secs(u64::from(d.lookup_timeout_secs?.get())),
            ))
        })
        .collect()
}

/// Wire real adapters from the environment into a context.
pub(crate) fn build(env: &Env) -> Result<Cx, CommandError> {
    Ok(Cx::new(parts(env)?).with_repos(env.repos().clone()))
}

/// Settles the Proxy supervisors recorded in the state store; one line per failure.
pub(crate) fn reconcile_proxies(env: &Env) -> Vec<String> {
    let Some(state_dir) = env.state_dir() else {
        return Vec::new();
    };
    let clock = match SystemClock::from_env(env) {
        Ok(clock) => Arc::new(clock),
        Err(e) => return vec![e.to_string()],
    };
    let config = match load_user(env) {
        Ok(config) => config,
        Err(e) => return vec![e.error.to_string()],
    };
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let host = DetachedSupervisor::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(SystemRng),
        Some(state_dir.clone()),
    )
    .keeping_nonce();
    adopt::reconcile(&host, proc.as_ref(), clock, &state_dir)
}

/// Real adapters from the environment, not yet assembled into a context.
fn parts(env: &Env) -> Result<CxParts, CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_user(env)?;
    let cwd = std::env::current_dir()
        .map_err(|e| CommandError::new(ErrorKind::Failed, format!("current directory: {e}")))?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let extra_env = HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect();
    let clock = Arc::new(clock);
    let surface: Arc<dyn Surface> = Arc::new(herdr(
        &proc,
        config.herdr_session(),
        extra_env,
        Arc::clone(&clock) as Arc<dyn Clock>,
    ));
    let driver: Arc<dyn Driver> = Arc::new(
        Crabbox::new(Arc::clone(&proc), Arc::clone(&clock) as Arc<dyn Clock>)
            .with_lookup_deadlines(lookup_deadlines(&config.drivers))
            .with_target(
                non_empty(env, CRABBOX_TARGET_ENV),
                non_empty(env, CRABBOX_WORK_ROOT_ENV),
            ),
    );
    let engine = BoxAgent::new(
        Arc::clone(&driver),
        Arc::clone(&surface),
        Arc::new(Claude),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(NonoRelease::new(
            Arc::new(UreqFetch),
            env.xdg("XDG_CACHE_HOME", ".cache", "moor"),
        )),
    );
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    let state_dir = env.state_dir();
    let proxy_host = Arc::new(DetachedSupervisor::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
        state_dir.clone(),
    ));
    Ok(CxParts {
        state_dir,
        clock,
        rng,
        driver,
        surface,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        forge: Arc::new(github(&proc, &config, env)),
        proxy_host,
        git: Git::new(Arc::clone(&proc)),
        config,
        creds: Creds::new(env.clone(), proc),
        cwd,
        prompt_window: prompt_window(env),
    })
}

/// Run the credential proxy for `task` until a stop signal; returns the exit code.
///
/// Setup failures are written to `stderr` as one `moor proxy: <error>` line.
pub fn serve_proxy(env: &Env, task: TaskId, stderr: impl Write + Send + 'static) -> u8 {
    let sink: Sink = Arc::new(Mutex::new(stderr));
    match run_proxy(env, task, &sink) {
        Ok(code) => code,
        Err(e) => {
            // The sink is the only channel left; a failed write has nowhere to go.
            let _ = writeln!(
                sink.lock().unwrap_or_else(PoisonError::into_inner),
                "moor proxy: {e}"
            );
            1
        }
    }
}

/// Builds the proxy's dependencies and serves; `Err` is a setup failure.
fn run_proxy(env: &Env, task: TaskId, sink: &Sink) -> Result<u8, String> {
    let parts = parts(env).map_err(|e| e.error.to_string())?;
    let forge = Arc::clone(&parts.forge);
    let clock = Arc::clone(&parts.clock);
    let proc = Arc::clone(&parts.proc);
    let cx = Cx::new(parts);
    let store = cx.store().map_err(|e| e.error.to_string())?;
    let state_dir = cx.state_dir().map_err(|e| e.error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    let sources = cx.config().credentials.clone();
    let reader = Creds::new(env.clone(), Arc::clone(&proc));
    let creds: CredsFn = Arc::new(move |name: &str| {
        sources.get(name).map_or_else(
            || Err(format!("credential {name} is not configured")),
            |src| reader.secret(src),
        )
    });
    let deps = Deps {
        store,
        driver: cx.driver(),
        forge,
        clock,
        rng: cx.rng(),
        creds,
        upstream_ca: None,
        upstream_dial: None,
        guard_exempt: Vec::new(),
        state_dir,
        boot_id: proc.boot_id(),
        stop,
        stderr: Arc::clone(sink),
    };
    Ok(serve(&deps, task))
}

/// The GitHub forge; `pass_env` names unset in `env` are skipped.
fn github(proc: &Arc<dyn Proc>, config: &UserConfig, env: &Env) -> GitHub {
    let gh = &config.github;
    let pairs = gh
        .pass_env
        .iter()
        .filter_map(|k| Some((k.clone(), env.get(k)?.to_owned())))
        .collect();
    GitHub::new(Arc::clone(proc), gh.token_cmd(), pairs)
}

/// The crabbox target (`linux`, `macos` or `windows`) the Box runs on.
const CRABBOX_TARGET_ENV: &str = "CRABBOX_TARGET";

/// The operator's work root on a static Mac Box.
const CRABBOX_WORK_ROOT_ENV: &str = "CRABBOX_STATIC_WORK_ROOT";

/// The variable's value, or `None` when it is unset or empty.
fn non_empty(env: &Env, key: &str) -> Option<String> {
    env.get(key).filter(|v| !v.is_empty()).map(str::to_owned)
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
mod tests;
