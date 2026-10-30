//! Composition root: the only module that names concrete adapters.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use moor_core::BoxId;
use signal_hook::consts::{SIGINT, SIGTERM};

use moor_core::config::{DriverConfig, SurfaceKind, UserConfig};
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
#[cfg(feature = "pty-surface")]
use crate::adapters::pty::PtySurface;
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

/// The host variables in [`HERDR_PASSTHROUGH`] that `env` sets.
fn herdr_env(env: &Env) -> Vec<(String, String)> {
    HERDR_PASSTHROUGH
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), env.get(k)?.to_owned())))
        .collect()
}

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

/// Opens the state store and returns `env` sharing it, so a store that every command would
/// refuse fails here.
pub(crate) fn open_store(env: Env) -> Result<Env, CommandError> {
    let Some(state_dir) = env.state_dir() else {
        return Ok(env);
    };
    let clock = SystemClock::from_env(&env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let store = Store::open(&state_dir.join(STORE_FILE), Arc::new(clock))?;
    Ok(env.sharing(store))
}

/// Ends every front Proxy lease in the store.
pub(crate) fn end_front_leases(env: &Env) -> Result<(), CommandError> {
    if let Some(store) = env.shared::<Store>() {
        store.purge_front_leases()?;
    }
    Ok(())
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

/// Settles the Proxy supervisors recorded in the store `env` shares; one line per failure.
pub(crate) fn reconcile_proxies(env: &Env) -> Vec<String> {
    let (Some(state_dir), Some(store)) = (env.state_dir(), env.shared::<Store>()) else {
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
        clock,
        Arc::new(SystemRng),
        Some(state_dir.clone()),
    )
    .sharing(Some(store.clone()));
    adopt::reconcile(&host, proc.as_ref(), store, &state_dir)
}

/// Whether the installed crabbox's `ssh` takes `--no-probe`. The controller wires a driver per
/// request and tick, so the answer is kept for the whole process: crabbox is asked once.
static NO_PROBE: LazyLock<Arc<OnceLock<bool>>> = LazyLock::new(Arc::default);

/// The process's one pty surface: its panes must outlive each request's wiring.
#[cfg(feature = "pty-surface")]
fn pty_surface(state_dir: Option<PathBuf>) -> Arc<dyn Surface> {
    static PTY: OnceLock<Arc<PtySurface>> = OnceLock::new();
    let pty = PTY.get_or_init(|| Arc::new(PtySurface::default().with_cwd(state_dir)));
    Arc::clone(pty) as Arc<dyn Surface>
}

/// The environment variable that picks the Surface when the config sets no `surface.kind`.
const SURFACE_ENV: &str = "MOOR_SURFACE";

/// The Surface kind `config` sets, else the one [`SURFACE_ENV`] names, else herdr.
fn surface_kind(config: &UserConfig, env: &Env) -> Result<SurfaceKind, CommandError> {
    if let Some(kind) = config.surface.kind {
        return Ok(kind);
    }
    match env.get(SURFACE_ENV).map(str::trim) {
        None | Some("") => Ok(SurfaceKind::Herdr),
        Some(name) => SurfaceKind::parse(name).ok_or_else(|| {
            CommandError::new(
                ErrorKind::Usage,
                format!("{SURFACE_ENV} is {name}, but it must be herdr or pty"),
            )
        }),
    }
}

/// The Surface of `kind`.
#[cfg_attr(
    feature = "pty-surface",
    allow(
        clippy::unnecessary_wraps,
        reason = "a build without the feature refuses `pty`"
    )
)]
fn surface(
    kind: SurfaceKind,
    config: &UserConfig,
    proc: &Arc<dyn Proc>,
    extra_env: Vec<(String, String)>,
    clock: Arc<dyn Clock>,
    state_dir: Option<PathBuf>,
) -> Result<Arc<dyn Surface>, CommandError> {
    match kind {
        SurfaceKind::Herdr => Ok(Arc::new(herdr(
            proc,
            config.herdr_session(),
            extra_env,
            clock,
            state_dir,
        ))),
        #[cfg(feature = "pty-surface")]
        SurfaceKind::Pty => Ok(pty_surface(state_dir)),
        #[cfg(not(feature = "pty-surface"))]
        SurfaceKind::Pty => Err(CommandError::new(
            ErrorKind::Usage,
            "the surface is pty, but this moor was built without the pty-surface feature",
        )),
    }
}

/// Real adapters from the environment, not yet assembled into a context, and the warning
/// when calls to a Box cannot share an ssh connection through control sockets in `sockets`.
fn parts(env: &Env, sockets: PathBuf) -> Result<(CxParts, Option<Warning>), CommandError> {
    let clock = SystemClock::from_env(env).map_err(|e| CommandError::new(ErrorKind::Usage, e))?;
    let config = load_user(env)?;
    let proc: Arc<dyn Proc> = Arc::new(SystemProc::new(env, &tool_paths(&config)));
    let clock = Arc::new(clock);
    let state_dir = env.state_dir();
    let surface = surface(
        surface_kind(&config, env)?,
        &config,
        &proc,
        herdr_env(env),
        Arc::clone(&clock) as Arc<dyn Clock>,
        state_dir.clone(),
    )?;
    let rng: Arc<dyn Rng> = Arc::new(SystemRng);
    let crabbox = Crabbox::new(
        Arc::clone(&proc),
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::clone(&rng),
    )
    .with_lookup_deadlines(lookup_deadlines(&config.drivers))
    .with_state_dir(state_dir.clone(), sockets)
    .with_runner_links(state_dir.as_deref())
    .sharing_no_probe(Arc::clone(&NO_PROBE));
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
    let proxy_host = Arc::new(
        DetachedSupervisor::new(
            Arc::clone(&proc),
            Arc::clone(&clock) as Arc<dyn Clock>,
            Arc::clone(&rng),
            state_dir.clone(),
        )
        .sharing(env.shared::<Store>().cloned()),
    );
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
    let shared = store.clone();
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
        machine_service: cx.config().machines.service.clone(),
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
