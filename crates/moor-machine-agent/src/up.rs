//! `up`: from a fresh host to a linked machine in one command, and the same end state on every
//! rerun. `down` takes the service away again.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::linked;
use crate::pair::{self, Ask};
use crate::pool::doctor_failures;
use crate::service::{Manager, Service};
use crate::settings::{self, Ensured};
use crate::{read_key, State, KEY};

/// How many seconds `up` waits for the service's first link.
const LINK_WAIT_SECS: u32 = 60;

/// What the machine offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// Boxes through this crabbox provider.
    Provider(String),
    /// Boxes through the provider crabbox is configured with.
    CrabboxDefault,
    /// No Boxes; it runs the `exec` scripts the server sends.
    Exec,
}

/// What `up` brings this machine to.
#[derive(Debug, Clone)]
pub struct Up {
    /// The moor server, such as `https://moor.example.com`.
    pub server: String,
    /// The machine's name, unique among its owner's machines.
    pub name: String,
    /// The machine's labels.
    pub labels: Vec<String>,
    /// How many Boxes may hold the machine at once.
    pub max_boxes: u16,
    /// What the machine offers.
    pub runtime: Runtime,
    /// The crabbox program.
    pub crabbox: PathBuf,
    /// The person's home, where crabbox's user config is.
    pub home: PathBuf,
}

/// Brings this machine to linked with `up.server`, keeping its key and state in `dir` and its
/// service with `manager`, and telling `say` each step; the last line to show.
///
/// # Errors
/// The server takes no machines, the provider is not ready, pairing fails, or the service does
/// not start or link.
pub fn up(
    up: &Up,
    dir: &Path,
    manager: &Manager,
    say: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let server = origin(&up.server);
    pair::check_server(&server)?;
    let (provider, crabbox) = match &up.runtime {
        Runtime::Exec => (None, None),
        named => {
            let crabbox = on_path(&up.crabbox)?;
            let provider = match named {
                Runtime::Provider(p) => p.clone(),
                _ => configured_provider(&crabbox)?,
            };
            apply_settings(&up.home, &provider, say);
            provider_ready(&crabbox, &provider)?;
            (Some(provider), Some(crabbox))
        }
    };
    let ask = Ask {
        name: up.name.clone(),
        labels: up.labels.clone(),
        max_boxes: up.max_boxes,
        provider,
    };
    let state = match paired(dir, &server)? {
        Some(state) => {
            say(&format!("{} is already paired with {server}", state.name));
            state
        }
        None => pair::pair(&server, &ask, dir, say)?,
    };
    let service = service(&ask, crabbox.as_deref(), dir, manager)?;
    let linked = dir.join(linked::FILE);
    manager.install(&service)?;
    if matches!(manager, Manager::Systemd { .. }) && !lingers() {
        say("run `loginctl enable-linger` so the agent keeps running after you log out");
    }
    for _ in 0..LINK_WAIT_SECS {
        if linked::live(&linked) {
            return Ok(format!(
                "linked: {}, {} Box slots",
                state.name, up.max_boxes
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err(format!(
        "the service started but has not linked in {LINK_WAIT_SECS}s; see {}",
        manager.log_hint(&service.log)
    ))
}

/// Stops and removes the service; this machine, whose state is in `dir`, stays paired.
///
/// # Errors
/// The service manager refuses.
pub fn down(manager: &Manager, dir: &Path) -> Result<String, String> {
    let removed = manager.remove()?;
    let _ = fs::remove_file(dir.join(linked::FILE));
    Ok(if removed {
        "stopped and removed the service; moor-machine-agent up starts it again".to_owned()
    } else {
        "no service to remove".to_owned()
    })
}

/// `server` as an origin: `https://` when no scheme is given, no trailing slash.
fn origin(server: &str) -> String {
    let server = server.trim_end_matches('/');
    if server.contains("://") {
        server.to_owned()
    } else {
        format!("https://{server}")
    }
}

/// `program` as a path the service can run without this shell's lookup.
fn on_path(program: &Path) -> Result<PathBuf, String> {
    if program.components().count() > 1 {
        return Ok(program.to_owned());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "{} is not on PATH; install crabbox and its provider first",
                program.display()
            )
        })
}

/// The provider crabbox's own config or environment selects.
fn configured_provider(crabbox: &Path) -> Result<String, String> {
    let out = Command::new(crabbox)
        .args(["doctor", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", crabbox.display()))?;
    let view: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    match view.get("provider").and_then(serde_json::Value::as_str) {
        Some(p) if !p.is_empty() => Ok(p.to_owned()),
        _ => Err(
            "crabbox has no provider selected; pass --runtime PROVIDER with one that \
             `crabbox providers` lists, or set `provider:` in crabbox's config"
                .to_owned(),
        ),
    }
}

/// Adds what warm starts need to crabbox's user config under `home`, saying what changed; a
/// failure only leaves this host's Boxes starting cold.
pub fn apply_settings(home: &Path, provider: &str, say: &mut dyn FnMut(&str)) {
    let path = settings::config_path(home);
    match settings::ensure(&path, provider) {
        Ok(Ensured::Added) => say(&format!(
            "turned on checkpoints in {}; set them to false there to opt out",
            path.display()
        )),
        Ok(Ensured::Kept) => {}
        Err(e) => say(&format!("cannot turn on checkpoints: {e}")),
    }
}

/// Asks crabbox whether `provider` can run Boxes here, showing its own fixes when not.
fn provider_ready(crabbox: &Path, provider: &str) -> Result<(), String> {
    let out = Command::new(crabbox)
        .args(["doctor", "--provider", provider])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", crabbox.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let (said, err) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let failed = doctor_failures(&said, &err);
    let lines = if failed.is_empty() {
        format!("{}\n{}", said.trim(), err.trim())
    } else {
        failed.join("\n")
    };
    Err(format!(
        "crabbox provider {provider} is not ready here:\n{}\nfix the above, then run moor-machine-agent up again",
        lines.trim()
    ))
}

/// The state of a pairing with `server` that left its key in `dir`.
fn paired(dir: &Path, server: &str) -> Result<Option<State>, String> {
    let Ok(text) = fs::read_to_string(dir.join("state.json")) else {
        return Ok(None);
    };
    let state: State = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} is not the agent's state: {e}",
            dir.join("state.json").display()
        )
    })?;
    Ok((state.server == server && read_key(&dir.join(KEY))?.is_some()).then_some(state))
}

/// The service that runs `run` for `ask` with this executable, serving the pool through
/// `crabbox` when there is one.
fn service(
    ask: &Ask,
    crabbox: Option<&Path>,
    dir: &Path,
    manager: &Manager,
) -> Result<Service, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let state_home = dir.parent().unwrap_or(dir).to_string_lossy().into_owned();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let log = manager
        .file()
        .parent()
        .and_then(Path::parent)
        .map_or_else(PathBuf::new, |lib| lib.join("Logs/moor-machine-agent.log"));
    let mut args = vec!["run".to_owned()];
    match (&ask.provider, crabbox) {
        (Some(provider), Some(crabbox)) => args.extend([
            "--runtime".to_owned(),
            provider.clone(),
            "--max-boxes".to_owned(),
            ask.max_boxes.to_string(),
            "--crabbox".to_owned(),
            crabbox.to_string_lossy().into_owned(),
        ]),
        _ => args.push("--allow-exec".to_owned()),
    }
    args.push("--auto-update".to_owned());
    Ok(Service {
        exe,
        args,
        env: vec![
            ("PATH".to_owned(), path.to_string_lossy().into_owned()),
            ("XDG_STATE_HOME".to_owned(), state_home),
        ],
        log,
    })
}

/// Whether the person's services outlive their login; unknown counts as yes.
fn lingers() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .stdin(Stdio::null())
        .output()
        .map_or(true, |out| {
            !String::from_utf8_lossy(&out.stdout).contains("Linger=no")
        })
}
