//! Composition root: the only module that names concrete adapters.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use moor_migrate::Outcome;

use moor_core::config::{DriverConfig, UserConfig};
use moor_core::seam::checkout::Checkout;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Proc;
use moor_core::seam::rng::Rng;
use moor_core::{ErrorKind, Warning};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::adapters::clock::SystemClock;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
pub(crate) use crate::adapters::proc::leave_signals_to_controller;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::adapters::rng::SystemRng;
use crate::adapters::sockets::socket_dir;
use crate::config_load::load_user;
use crate::cx::{Cx, CxParts, MachineLinks};
use crate::env::{Env, CONTROLLER_ALIVE};
use crate::error::CommandError;
use crate::store::controller_lock::{Claim, Claimant, Lost};
use crate::store::{Database, Store};
use crate::vault::Vault;

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

/// Opens the state store and returns `env` sharing it, so a store that every command would
/// refuse fails here.
pub(crate) fn open_store(env: Env) -> Result<Env, CommandError> {
    let Some(state_dir) = env.state_dir() else {
        return Ok(env);
    };
    let clock = SystemClock::from_env(&env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let store = Store::open_claimed(
        &state_dir,
        Database::from_env(&env)?.as_ref(),
        Arc::new(clock),
    )?;
    Ok(env.sharing(store))
}

/// Waits until this controller may run on `env`'s store, which it then may until the process
/// exits. False means `stopped` said to give up. A store in a file waits for nothing here, as its
/// file lock serves. This comes before any migration, so a waiter never changes the schema under
/// a live holder.
pub(crate) fn claim_controller(
    env: &Env,
    stopped: &dyn Fn(Duration) -> bool,
    note: &dyn Fn(&str),
    lost: Lost,
) -> Result<bool, CommandError> {
    let Some(state_dir) = env.state_dir() else {
        return Ok(true);
    };
    let claimant = Claimant {
        me: env.hostname().map_or_else(
            || format!("controller-{}", std::process::id()),
            str::to_owned,
        ),
        alive: state_dir.join(CONTROLLER_ALIVE),
        stopped,
        note,
        lost,
    };
    let claim = claimant.claim(Database::from_env(env)?.as_ref())?;
    Ok(matches!(claim, Claim::Held))
}

/// The warning for a store a newer build migrated, which this build opened unchanged.
pub(crate) fn schema_warning(env: &Env) -> Option<String> {
    let Outcome::Ahead { found, known } = env.shared::<Store>()?.schema() else {
        return None;
    };
    Some(ahead_warning(found, known))
}

/// The warning for a store a newer build migrated to `found`, past this build's `known`.
fn ahead_warning(found: i64, known: i64) -> String {
    format!(
        "warning: the state database is at migration {found} and this moor knows up to {known}; \
         leaving it unchanged"
    )
}

/// Opens the store `env` names, which migrates it; the line to print, or why it failed.
///
/// A deploy step runs it before any controller starts. A store a newer build migrated is left
/// unchanged, and the line warns so.
pub fn migrate_store(env: &Env) -> Result<String, String> {
    let state_dir = env
        .state_dir()
        .ok_or("no state directory: set MOOR_STATE_DIR, XDG_STATE_HOME or HOME")?;
    let clock = SystemClock::from_env(env).map_err(|e| e.to_string())?;
    let database = Database::from_env(env).map_err(|e| e.to_string())?;
    let store = Store::open_in(&state_dir, database.as_ref(), Arc::new(clock))
        .map_err(|e| e.to_string())?;
    Ok(match store.schema() {
        Outcome::Current => format!("migrated the store in {}", store.location()),
        Outcome::Ahead { found, known } => ahead_warning(found, known),
    })
}

/// Ends the front Proxy leases of every front that does not run elsewhere.
pub(crate) fn end_front_leases(env: &Env) -> Result<(), CommandError> {
    if let Some(store) = env.shared::<Store>() {
        store.purge_front_leases(env.remote_fronts())?;
    }
    Ok(())
}

/// See [`crate::keys::settle`]; an env with no store or state dir has none.
pub(crate) fn settle_keys(env: &Env, vault: &Vault, front_ca: bool) -> Result<(), String> {
    let (Some(store), Some(state)) = (env.shared::<Store>(), env.state_dir()) else {
        return Ok(());
    };
    crate::keys::settle(store, vault, &state, front_ca)
}

/// The socket of front `ns`'s Proxy for `state_dir`.
pub(crate) fn front_proxy_socket(state_dir: &Path, ns: &str) -> PathBuf {
    crate::proxy::fronts::socket_path(state_dir, ns)
}

/// Creates the directory of front `ns`'s Proxy socket for `state_dir`, so the front's shim can
/// open it before the Proxy binds the socket.
pub(crate) fn prepare_front_proxy_socket(state_dir: &Path, ns: &str) -> Result<(), String> {
    let owner = rustix::process::getuid().as_raw();
    crate::proxy::front::FrontSocket::of_front(owner, state_dir, ns).prepare()
}

/// The front Proxy's CA certificate, made when missing.
pub(crate) fn front_proxy_ca(state_dir: &Path) -> std::io::Result<PathBuf> {
    crate::proxy::fronts::ca_cert(state_dir)
}

/// Whether the installed crabbox's `ssh` takes `--no-probe`. The controller wires a driver per
/// request and tick, so the answer is kept for the whole process: crabbox is asked once.
static NO_PROBE: LazyLock<Arc<OnceLock<bool>>> = LazyLock::new(Arc::default);

/// Real adapters from the environment, not yet assembled into a context, and the warning
/// when calls to a Box cannot share an ssh connection through control sockets in `sockets`.
fn parts(env: &Env, sockets: PathBuf) -> Result<(CxParts, Option<Warning>), CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_user(env)?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let clock = Arc::new(clock);
    let state_dir = env.state_dir();
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    let links = state_dir.as_deref().map(|dir| {
        links::under(
            dir,
            env.shared::<Store>().cloned(),
            Arc::clone(&clock) as Arc<dyn Clock>,
            env.get(crate::env::MACHINE_SERVICE_KEY),
        )
    });
    let crabbox = Crabbox::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
    )
    .with_lookup_deadlines(lookup_deadlines(&config.drivers))
    .with_state_dir(state_dir.clone(), sockets)
    .with_checkpoint_dir(env.xdg("XDG_CACHE_HOME", ".cache", ""))
    .with_pool_hosts(links.clone().map(|l| l as _))
    .sharing_no_probe(Arc::clone(&NO_PROBE));
    let link_warning = crabbox.link_warning();
    let driver: Arc<dyn Driver> = Arc::new(crabbox);
    let engine = BoxAgent::new(Arc::clone(&driver), Arc::new(Claude));
    let proxy_host = proxy_host(env, &driver, &(Arc::clone(&clock) as Arc<dyn Clock>), &rng);
    let parts = CxParts {
        state_dir,
        clock,
        rng,
        driver,
        engine: Arc::new(engine),
        proc: Arc::clone(&proc),
        forge: Arc::new(GitHub::new()),
        proxy_host,
        machine_links: links.map(|l| l as Arc<dyn MachineLinks>),
        config,
        env: env.clone(),
        prompt_window: prompt_window(env),
    };
    Ok((parts, link_warning))
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
pub(crate) fn tool_paths(config: &UserConfig) -> ToolPaths {
    let t = &config.tools;
    ToolPaths {
        crabbox: t.crabbox.clone(),
        herdr: t.herdr.clone(),
        claude: t.claude.clone(),
        ssh: t.ssh.clone(),
        git: t.git.clone(),
    }
}

mod egress;
mod keepers;
mod ledger;
mod links;
mod store_copy;
pub use store_copy::copy_store;

pub(crate) use egress::prepare_egress;
use keepers::proxy_host;
pub(crate) use keepers::{end_keepers, keepers_tunnel, reconcile_proxies};
pub(crate) use ledger::{serve_egress_ledger, serve_ledger};

#[cfg(test)]
mod tests;
