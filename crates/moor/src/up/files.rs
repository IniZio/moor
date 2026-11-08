//! The two config files `up` converges: the controller's and the user's, one fixed path each.

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use moor_core::config::{parse_controller, parse_user};

use crate::config_load::user_path;
use crate::doctor::Check;
use crate::env::Env;

/// The name of the node `up` makes.
pub(crate) const NODE: &str = "local";

/// Memory one Box is counted at, and memory kept for the host, in MiB.
const MIB_PER_BOX: u64 = 4096;

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

/// The local node `up` adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    /// The crabbox provider.
    pub runtime: String,
    /// The cap on live Boxes.
    pub max_boxes: NonZeroU32,
    /// Whether `--max-boxes` named the cap.
    pub asked: bool,
}

/// One Box per [`MIB_PER_BOX`] of memory beyond as much again for the host, and at least one.
pub(crate) fn max_boxes(mem_total_mib: u64) -> NonZeroU32 {
    let n = mem_total_mib.saturating_sub(MIB_PER_BOX) / MIB_PER_BOX;
    NonZeroU32::new(u32::try_from(n).unwrap_or(u32::MAX)).unwrap_or(NonZeroU32::MIN)
}

/// `MemTotal` from this host's `/proc/meminfo`, in MiB.
pub(crate) fn host_mem_mib() -> Option<u64> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kib / 1024)
}

/// The user config text after `up`: `existing` with a `controller` and a `nodes` section added
/// where it has none, or `None` when it needs no change.
///
/// # Errors
/// A config that names another controller, or whose node differs from what `--max-boxes` asks.
pub(crate) fn converge_user(
    env: &Env,
    file: &str,
    existing: Option<&str>,
    socket: &Path,
    node: &Node,
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
    match config.nodes.get(NODE) {
        Some(have) if node.asked && have.max_boxes != Some(node.max_boxes) => {
            return Err(Check::fail(
                "config",
                format!("{file} already has node {NODE}"),
                format!("set nodes.{NODE}.max_boxes to {} there", node.max_boxes),
            ))
        }
        None if config.nodes.is_empty() && !has_key("nodes") => section(
            &mut out,
            &format!(
                "nodes:\n  {NODE}:\n    runtime: {}\n    max_boxes: {}\n",
                node.runtime, node.max_boxes
            ),
        ),
        _ => {}
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

/// Adds the controller and the local node to the user config, keeping what it already says.
pub(crate) fn user(env: &Env, layout: &Layout, node: &Node) -> Result<Check, Check> {
    let path = &layout.user_config;
    let file = path.display().to_string();
    let existing = fs::read_to_string(path).ok();
    if let Some(text) = converge_user(env, &file, existing.as_deref(), &layout.socket, node)? {
        write(path, &text)?;
    }
    let text = fs::read_to_string(path).unwrap_or_default();
    let config = parse_user(&file, &text)
        .map_err(|e| Check::fail("config", e.to_string(), format!("fix {file}")))?;
    let nodes: Vec<String> = config
        .nodes
        .iter()
        .map(|(name, n)| {
            let cap = n
                .max_boxes
                .map_or_else(|| "no cap".to_owned(), |m| format!("max {m} Boxes"));
            format!("{name} ({}, {cap})", n.runtime)
        })
        .collect();
    Ok(Check::ok(
        "config",
        format!("{file}: node {}", nodes.join(", ")),
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
