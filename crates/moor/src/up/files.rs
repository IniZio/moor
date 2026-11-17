//! The two config files `up` converges: the controller's and the user's, one fixed path each.

use std::fs;
use std::path::{Path, PathBuf};

use moor_core::config::{parse_controller, parse_user};

use crate::config_load::user_path;
use crate::doctor::Check;
use crate::env::Env;

/// The name of the node `up` makes.
pub(crate) const NODE: &str = "local";

/// Where everything `up` writes lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The controller config: `<config dir>/moor/controller.yaml`.
    pub controller_config: PathBuf,
    /// The user config the CLI and the controller both read.
    pub user_config: PathBuf,
    /// The controller's state directory.
    pub state_dir: PathBuf,
    /// The controller's socket.
    pub socket: PathBuf,
}

impl Layout {
    /// The `HOME` of the local node's machine agent, under the controller's state.
    pub(crate) fn agent_home(&self) -> PathBuf {
        self.state_dir.join("agent-home")
    }

    /// The machine runner's own state directory.
    pub(crate) fn runner_state(&self) -> PathBuf {
        self.state_dir.join("machine-runner")
    }

    /// The socket the controller takes the machine runner on.
    pub(crate) fn runner_socket(&self) -> PathBuf {
        crate::runner_dir::of(&self.state_dir).join(crate::runner_dir::SOCKET)
    }

    /// The paths for `env`, with the socket and state dir an existing controller config sets.
    pub(crate) fn read(env: &Env) -> Result<Self, Check> {
        let no_home = || Check::fail("config", "HOME is unset", "run moor up as a login user");
        let controller_config = env
            .xdg("XDG_CONFIG_HOME", ".config", "moor/controller.yaml")
            .ok_or_else(no_home)?;
        let user_config = user_path(env).ok_or_else(no_home)?;
        let existing = match fs::read_to_string(&controller_config) {
            Ok(text) => Some(
                parse_controller(&controller_config.display().to_string(), &text).map_err(|e| {
                    let fix = format!("fix or remove {}", controller_config.display());
                    Check::fail("config", e.to_string(), fix)
                })?,
            ),
            Err(_) => None,
        };
        let expand = |s: &String| env.expand_home(Path::new(s));
        let state_dir = existing
            .as_ref()
            .and_then(|c| c.state_dir.as_ref().map(expand))
            .or_else(|| env.state_dir())
            .ok_or_else(no_home)?;
        let socket = existing
            .as_ref()
            .and_then(|c| c.socket.as_ref().map(expand))
            .unwrap_or_else(|| state_dir.join("controller.sock"));
        Ok(Self {
            controller_config,
            user_config,
            state_dir,
            socket,
        })
    }
}

/// The controller config `up` writes when there is none.
pub(crate) fn controller_yaml(layout: &Layout) -> String {
    format!(
        "socket: {}\nstate_dir: {}\n",
        layout.socket.display(),
        layout.state_dir.display()
    )
}

/// Writes the controller config when it is missing; an existing one is kept as it is.
pub(crate) fn controller(layout: &Layout) -> Result<Check, Check> {
    let path = &layout.controller_config;
    if !path.exists() {
        write(path, &controller_yaml(layout))?;
    }
    Ok(Check::ok(
        "config",
        format!("{}: socket {}", path.display(), layout.socket.display()),
    ))
}

/// moor-gate's own `--box-listen`, where the Proxy beside the controller sends `moor.moor.internal`.
const GATE_BOX_LISTEN: &str = "127.0.0.1:8444";

/// The user config text after `up`: `existing` with a `controller` section, and a `moor_service`
/// routed to the gate on this host, added where it has none; `None` when it needs no change.
///
/// # Errors
/// A config that names another controller, or that still holds the `local` node an earlier
/// `moor up` wrote, which the Docker node of the same name replaces.
pub(crate) fn converge_user(
    env: &Env,
    file: &str,
    existing: Option<&str>,
    socket: &Path,
) -> Result<Option<String>, Check> {
    let text = existing.unwrap_or_default();
    let config = parse_user(file, text)
        .map_err(|e| Check::fail("config", e.to_string(), format!("fix {file}")))?;
    let mut out = text.to_owned();
    let has_key = |key: &str| text.lines().any(|l| l.starts_with(&format!("{key}:")));
    match (&config.controller.socket, &config.controller.ssh) {
        (Some(s), _) if env.expand_home(s) == socket => {}
        (None, None) if !has_key("controller") => {
            section(
                &mut out,
                &format!("controller:\n  socket: {}\n", socket.display()),
            );
        }
        _ => {
            return Err(Check::fail(
                "config",
                format!("{file} names another controller"),
                format!("set controller.socket to {} there", socket.display()),
            ))
        }
    }
    if config.nodes.contains_key(NODE) {
        return Err(Check::fail(
            "config",
            format!("{file} still has the node {NODE} an earlier moor up wrote"),
            format!("delete nodes.{NODE} from {file}; moor up adds the Docker node of that name"),
        ));
    }
    if config.moor_service.is_none() && !has_key("moor_service") {
        section(
            &mut out,
            &format!("moor_service:\n  upstream: {GATE_BOX_LISTEN}\n"),
        );
    }
    Ok((existing != Some(out.as_str())).then_some(out))
}

/// Appends a top-level `block` to `out`, on a line of its own.
fn section(out: &mut String, block: &str) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
}

/// Adds the controller to the user config, keeping what it already says.
pub(crate) fn user(env: &Env, layout: &Layout) -> Result<Check, Check> {
    let path = &layout.user_config;
    let file = path.display().to_string();
    let existing = fs::read_to_string(path).ok();
    if let Some(text) = converge_user(env, &file, existing.as_deref(), &layout.socket)? {
        write(path, &text)?;
    }
    Ok(Check::ok(
        "config",
        format!("{file}: controller socket {}", layout.socket.display()),
    ))
}

/// Writes `text` to `path` through a temporary file, making its directory.
pub(crate) fn write(path: &Path, text: &str) -> Result<(), Check> {
    let fail = |e: std::io::Error| {
        Check::fail(
            "config",
            format!("cannot write {}: {e}", path.display()),
            "make the directory writable",
        )
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(fail)?;
    fs::rename(&tmp, path).map_err(fail)
}
