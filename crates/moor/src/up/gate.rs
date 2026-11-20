//! The gate `up` runs for the Boxes on this host: `moor-gate` with its public listener off, so it
//! serves only `report_step`. A planned workflow run needs it, and it needs the shared service key
//! so that no other process on the host can name a Box.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use moor_core::config::parse_user;

use super::files::{has_key, Layout};
use super::node::{binary, INSTALL};
use super::service;
use crate::doctor::Check;
use crate::env::Env;
use crate::front_dir;
use crate::proxy::token::base64url;

/// moor-gate's own `--box-listen`, where the Proxy beside the controller sends `moor.moor.internal`.
pub(super) const BOX_LISTEN: &str = "127.0.0.1:8444";

/// The namespace the gate speaks as, which names its front.
pub(super) const NAMESPACE: &str = "authgear";

/// The front `up` gives the controller for the gate, allowed to report steps.
pub(super) const FRONT: &str = "fronts:\n  authgear:\n    step_reports: true\n";

/// The gate's executable.
const BIN: &str = "moor-gate";
/// The file in the state dir that holds the shared service key, for the controller and the gate.
const KEY_FILE: &str = "service.env";
/// The variable the key is set in, which the Proxy and the gate both read.
const KEY_VAR: &str = "MOOR_SERVICE_KEY";

/// What `up` does about the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Gate {
    /// Run it as a unit.
    Run(Plan),
    /// A gate of the person's own serves the Boxes, for this reason.
    Left(&'static str),
}

/// What the gate's unit needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Plan {
    /// The gate's executable.
    pub exe: PathBuf,
    /// The file holding the service key.
    pub key_file: PathBuf,
    /// The controller's front socket the gate speaks through.
    pub front_socket: PathBuf,
}

impl Gate {
    /// Whether the controller config needs the gate's front.
    pub(super) const fn runs(&self) -> bool {
        matches!(self, Self::Run(_))
    }

    /// The fronts whose sockets `up` holds for the controller: the gate's, when it runs it.
    pub(super) const fn fronts(&self) -> &'static [&'static str] {
        match self {
            Self::Run(_) => &[NAMESPACE],
            Self::Left(_) => &[],
        }
    }
}

/// Decides, before `up` writes anything, whether it runs the gate: not when the user config
/// routes `moor_service` elsewhere, and not when the controller config has fronts that `up` did
/// not write, which means the person runs their own gate. Whatever `up` wrote earlier, the
/// key file or the gate unit, marks the fronts as its own, so an `up` that stopped part way
/// resumes with the gate.
///
/// # Errors
/// The user config is invalid, or `moor-gate` is installed nowhere.
pub(super) fn decide(env: &Env, layout: &Layout, unit_dir: &Path) -> Result<Gate, Check> {
    let file = layout.user_config.display().to_string();
    let user = fs::read_to_string(&layout.user_config).unwrap_or_default();
    let config = parse_user(&file, &user)
        .map_err(|e| Check::fail("config", e.to_string(), format!("fix {file}")))?;
    if config
        .moor_service
        .is_some_and(|s| s.upstream.as_str() != BOX_LISTEN)
    {
        return Ok(Gate::Left("moor_service names a listener of your own"));
    }
    let controller = fs::read_to_string(&layout.controller_config).unwrap_or_default();
    let key_file = layout.state_dir.join(KEY_FILE);
    let wrote_it = key_file.exists() || unit_dir.join(service::GATE).exists();
    if has_key(&controller, "fronts") && !wrote_it {
        return Ok(Gate::Left("controller.yaml sets fronts of its own"));
    }
    let exe = binary(env, BIN).ok_or_else(|| {
        Check::fail(
            "gate",
            format!("{BIN} is neither beside moor nor on PATH"),
            INSTALL,
        )
    })?;
    Ok(Gate::Run(Plan {
        exe,
        key_file,
        front_socket: front_dir::of(&layout.state_dir, NAMESPACE).join(front_dir::FRONT_SOCKET),
    }))
}

/// The gate's unit text: no public listener, the Box listener where `moor_service` routes, and
/// the service key from its file.
pub(super) fn unit(plan: &Plan) -> String {
    let args: Vec<String> = [
        ("--listen", "off".to_owned()),
        ("--box-listen", BOX_LISTEN.to_owned()),
        ("--front-socket", plan.front_socket.display().to_string()),
    ]
    .into_iter()
    .flat_map(|(flag, value)| [flag.to_owned(), value])
    .collect();
    service::process_unit(
        "moor gate: serves report_step to the Boxes on this host",
        service::SERVICE,
        &plan.exe,
        &args,
        &[],
        Some(&plan.key_file),
    )
}

/// The service key file, made with a new random key when it is missing or holds none. A rerun
/// keeps the key, so no unit text changes.
///
/// # Errors
/// The file cannot be written.
pub(super) fn keep_key(plan: &Plan) -> Result<(), Check> {
    let path = &plan.key_file;
    let kept = fs::read_to_string(path).is_ok_and(|t| {
        t.strip_prefix(&format!("{KEY_VAR}="))
            .is_some_and(|key| !key.trim().is_empty())
    });
    if kept {
        return Ok(());
    }
    let fail = |why: String| {
        Check::fail(
            "gate",
            format!("cannot keep the service key in {}: {why}", path.display()),
            "make the state directory writable",
        )
    };
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| fail(e.to_string()))?;
    let draft = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&draft)
        .map_err(|e| fail(e.to_string()))?;
    writeln!(file, "{KEY_VAR}={}", base64url(&bytes)).map_err(|e| fail(e.to_string()))?;
    drop(file);
    fs::rename(&draft, path).map_err(|e| fail(e.to_string()))
}

/// Installs and starts the gate's unit when `gate` runs it; one check either way.
///
/// # Errors
/// The unit cannot be installed.
pub(super) fn converge(dir: &Path, gate: &Gate) -> Result<Check, Check> {
    match gate {
        Gate::Run(plan) => {
            service::install_one(dir, service::GATE, &unit(plan), false)?;
            Ok(Check::ok(
                "gate",
                format!(
                    "{} serves report_step to the Boxes at {BOX_LISTEN}",
                    service::GATE
                ),
            ))
        }
        Gate::Left(why) => Ok(Check::ok("gate", format!("not started: {why}"))),
    }
}
