//! Composition root: the only module that names concrete adapters.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::BoxId;
use signal_hook::consts::{SIGINT, SIGTERM};

use moor_core::config::{DriverConfig, UserConfig};
use moor_core::seam::checkout::Checkout;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, Warning};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::{GhRelease, UreqFetch};
use crate::adapters::box_agent::BoxAgent;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
pub(crate) use crate::adapters::proc::leave_signals_to_controller;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::adapters::sockets::socket_dir;
use crate::config_load::load_user;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::error::CommandError;
use crate::proxy::adopt;
use crate::proxy::detached::DetachedSupervisor;
use crate::proxy::nono::capture::SecretsFn;
use crate::proxy::{serve, Deps, Sink};
use crate::store::{Store, STORE_FILE};

/// Host variables herdr needs to find its socket and terminal.
const HERDR_PASSTHROUGH: [&str; 5] = [
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "TERM",
];

/// The herdr adapter, numbering its reports from `clock` and opening Workspaces in `state_dir`.
fn herdr(
    proc: &Arc<dyn Proc>,
    session: &str,
    extra_env: Vec<(String, String)>,
    clock: Arc<dyn Clock>,
    state_dir: Option<PathBuf>,
) -> Herdr {
    let h = Herdr::new(Arc::clone(proc), session.to_owned(), extra_env).with_clock(clock);
    match state_dir {
        Some(dir) => h.with_workspace_dir(dir),
        None => h,
    }
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
    build_with_sockets(env, socket_dir())
}

/// [`build`], with each Box's ssh control socket in `sockets`.
fn build_with_sockets(env: &Env, sockets: PathBuf) -> Result<Cx, CommandError> {
    let (parts, warning) = parts(env, sockets)?;
    let mut cx = Cx::new(parts);
    if let Some(key) = env.vault_key() {
        cx = cx.with_vault_key(key.to_path_buf());
    }
    if let Some(w) = warning {
        cx.warn(w);
    }
    Ok(cx)
}

/// The caller's checkout as the CLI reads it, with host git on `PATH`.
pub(crate) fn host_checkout(env: &Env) -> impl Checkout {
    Git::new(Arc::new(SystemProc::new(env, &ToolPaths::default())))
}

/// Opens the state store once, so a store that every command would refuse fails here.
pub(crate) fn open_store(env: &Env) -> Result<(), CommandError> {
    let Some(state_dir) = env.state_dir() else {
        return Ok(());
    };
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    Store::open(&state_dir.join(STORE_FILE), Arc::new(clock))?;
    Ok(())
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
    );
    adopt::reconcile(&host, proc.as_ref(), clock, &state_dir)
}

/// Real adapters from the environment, not yet assembled into a context, and the warning
/// when calls to a Box cannot share an ssh connection through control sockets in `sockets`.
fn parts(env: &Env, sockets: PathBuf) -> Result<(CxParts, Option<Warning>), CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_user(env)?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let extra_env = HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect();
    let clock = Arc::new(clock);
    let state_dir = env.state_dir();
    let surface: Arc<dyn Surface> = Arc::new(herdr(
        &proc,
        config.herdr_session(),
        extra_env,
        Arc::clone(&clock) as Arc<dyn Clock>,
        state_dir.clone(),
    ));
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    let crabbox = Crabbox::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
    )
    .with_lookup_deadlines(lookup_deadlines(&config.drivers))
    .with_state_dir(state_dir.clone(), sockets);
    let link_warning = crabbox.link_warning();
    let driver: Arc<dyn Driver> = Arc::new(crabbox);
    let engine = BoxAgent::new(
        Arc::clone(&driver),
        Arc::clone(&surface),
        Arc::new(Claude),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(GhRelease::new(
            Arc::new(UreqFetch),
            env.xdg("XDG_CACHE_HOME", ".cache", "moor"),
        )),
    );
    let proxy_host = Arc::new(DetachedSupervisor::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
        state_dir.clone(),
    ));
    let parts = CxParts {
        state_dir,
        clock,
        rng,
        driver,
        surface,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        forge: Arc::new(GitHub::new()),
        proxy_host,
        config,
        env: env.clone(),
        prompt_window: prompt_window(env),
    };
    Ok((parts, link_warning))
}

/// Run the credential proxy for `box_id` until a stop signal; returns the exit code.
///
/// Setup failures are written to `stderr` as one `moor proxy: <error>` line.
pub fn serve_proxy(env: &Env, box_id: BoxId, stderr: impl Write + Send + 'static) -> u8 {
    let sink: Sink = Arc::new(Mutex::new(stderr));
    match run_proxy(env, box_id, &sink) {
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
fn run_proxy(env: &Env, box_id: BoxId, sink: &Sink) -> Result<u8, String> {
    let sockets = socket_dir();
    let (parts, _) = parts(env, sockets.clone()).map_err(|e| e.error.to_string())?;
    let clock = Arc::clone(&parts.clock);
    let proc = Arc::clone(&parts.proc);
    let cx = Cx::new(parts);
    let store = cx.store().map_err(|e| e.error.to_string())?;
    let state_dir = cx.state_dir().map_err(|e| e.error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    let shared = Arc::new(
        Store::open(&state_dir.join(STORE_FILE), Arc::clone(&clock)).map_err(|e| e.to_string())?,
    );
    let secrets: SecretsFn = Arc::new(move |vault, user, name| {
        shared
            .user_secret(vault, user, name)
            .map_err(|e| e.to_string())
    });
    let deps = Deps {
        store,
        driver: cx.driver(),
        clock,
        rng: cx.rng(),
        secrets,
        upstream_ca: None,
        upstream_dial: None,
        guard_exempt: Vec::new(),
        state_dir,
        sockets: &sockets,
        owner: rustix::process::getuid().as_raw(),
        boot_id: proc.boot_id(),
        stop,
        stderr: Arc::clone(sink),
    };
    Ok(serve(&deps, box_id))
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
