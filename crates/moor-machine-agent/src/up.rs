//! `up`: from a fresh host to a linked machine in one command, and the same end state on every
//! rerun. `down` takes the service away again.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use moor_core::config::BOXES_HOST;
use std::time::Duration;

use moor_client::{kvm, rootless};

use crate::linked;
use crate::pair::{self, Ask, Token};
use crate::service::{Manager, Service};
use crate::{read_key, Host, State, KEY};

/// The kind of Boxes `up` offers.
const DOCKER: &str = "docker";

#[cfg(test)]
mod tests;

/// How many seconds `up` waits for the service's first link.
const LINK_WAIT_SECS: u32 = 60;

/// What the machine offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// Boxes this machine makes itself, of this kind, from these images (the release's own when
    /// left out).
    Boxes {
        /// How the machine makes Boxes; `docker`.
        kind: String,
        /// The agent place's image.
        agent_image: Option<String>,
        /// The work place's image.
        work_image: Option<String>,
        /// How long cache volumes may sit unused before they are deleted; none is the default.
        cache_max_age: Option<std::time::Duration>,
    },
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
    /// What the machine offers.
    pub runtime: Runtime,
    /// A portal-minted token that pairs the machine with no confirm page; used only when the
    /// machine is not paired yet.
    pub token: Option<Token>,
}

/// Brings this machine to linked with `up.server`, keeping its key and state in `dir` and its
/// service with `manager`, and telling `say` each step; the last line to show.
///
/// # Errors
/// The server takes no machines, Docker is not usable, pairing fails, or the service does
/// not start or link.
pub fn up(
    up: &Up,
    dir: &Path,
    manager: &Manager,
    say: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let server = origin(&up.server);
    pair::check_server(&server)?;
    let pool = match &up.runtime {
        Runtime::Exec => None,
        Runtime::Boxes { kind, .. } => {
            Host::named(kind, dir)?;
            if kind == DOCKER {
                docker_ready()?;
                trust_docker(say)?;
            }
            Some(BOXES_HOST.to_owned())
        }
    };
    let ask = Ask {
        name: up.name.clone(),
        labels: up.labels.clone(),
        pool,
    };
    let state = match paired(dir, &server, &ask)? {
        Some(state) => {
            say(&format!("{} is already paired with {server}", state.name));
            state
        }
        None => pair::pair(&server, &ask, up.token.as_ref(), dir, say)?,
    };
    let service = service(&up.runtime, dir, manager)?;
    let linked = dir.join(linked::FILE);
    manager.install(&service)?;
    if matches!(manager, Manager::Systemd { .. }) && !lingers() {
        say("run `loginctl enable-linger` so the agent keeps running after you log out");
    }
    for _ in 0..LINK_WAIT_SECS {
        if linked::live(&linked) {
            return Ok(format!("linked: {}", state.name));
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

/// The state of a pairing with `server` for `ask` that left its key in `dir`; none when the
/// machine was paired to offer something else, such as the crabbox pool an earlier release
/// offered, so that pairing again replaces what the server recorded.
fn paired(dir: &Path, server: &str, ask: &Ask) -> Result<Option<State>, String> {
    let Ok(text) = fs::read_to_string(dir.join("state.json")) else {
        return Ok(None);
    };
    let state: State = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} is not the agent's state: {e}",
            dir.join("state.json").display()
        )
    })?;
    let current = state.server == server && state.pool == ask.pool;
    Ok((current && read_key(&dir.join(KEY))?.is_some()).then_some(state))
}

/// The service that runs `run` for `runtime` with this executable.
fn service(runtime: &Runtime, dir: &Path, manager: &Manager) -> Result<Service, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let state_home = dir.parent().unwrap_or(dir).to_string_lossy().into_owned();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let log = manager
        .file()
        .parent()
        .and_then(Path::parent)
        .map_or_else(PathBuf::new, |lib| lib.join("Logs/moor-machine-agent.log"));
    let mut args = vec!["run".to_owned()];
    match runtime {
        Runtime::Exec => args.push("--allow-exec".to_owned()),
        Runtime::Boxes {
            kind,
            agent_image,
            work_image,
            cache_max_age,
        } => {
            if kind != DOCKER {
                args.extend(["--boxes".to_owned(), kind.clone()]);
            }
            for (flag, image) in [("--agent-image", agent_image), ("--work-image", work_image)] {
                if let Some(image) = image {
                    args.extend([flag.to_owned(), image.clone()]);
                }
            }
            if let Some(age) = cache_max_age {
                args.extend(["--cache-max-age".to_owned(), format!("{}s", age.as_secs())]);
            }
        }
    }
    args.push("--auto-update".to_owned());
    Ok(Service {
        exe,
        args,
        env: [
            ("PATH".to_owned(), path.to_string_lossy().into_owned()),
            ("XDG_STATE_HOME".to_owned(), state_home),
        ]
        .into_iter()
        .chain(docker_env(&|name| std::env::var(name).ok()))
        .collect(),
        log,
    })
}

/// Checks that this person can run Docker, showing what `docker` said when not.
fn docker_ready() -> Result<(), String> {
    let out = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            format!("cannot run docker: {e}; install Docker, then run moor-machine-agent up again")
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "Docker does not work for this user:\n{}\nfix it, such as by pointing DOCKER_HOST at your \
         rootless daemon, then run moor-machine-agent up again",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// The variables that point the person's `docker` at a daemon, for the service to keep: it
/// would otherwise reach the system daemon, whatever the person chose.
fn docker_env(get: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    ["DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG"]
        .into_iter()
        .filter_map(|name| Some((name.to_owned(), get(name).filter(|v| !v.is_empty())?)))
        .collect()
}

/// Says what Docker Boxes are on this daemon, after checking it is rootless Docker set up for them
/// and that Boxes can open the KVM device; says every check that fails.
fn trust_docker(say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let line = |f: &rootless::Failure| format!("{}: {}; fix: {}", f.check, f.detail, f.fix);
    if !rootless::detect() {
        return Err(line(&rootless::refusal()));
    }
    let set = std::env::var(kvm::DEVICE_VAR).ok();
    let device = kvm::device(set.as_deref());
    if let Some(f) = kvm::refusal(device, kvm::mode_of(device)) {
        return Err(line(&f));
    }
    let var = |name: &str| std::env::var(name).ok();
    let facts = rootless::Facts::read(
        var("PATH").as_deref(),
        var("USER").as_deref(),
        var(rootless::ROOT_VAR).as_deref(),
    );
    let failed = rootless::failures(&facts, false);
    if !failed.is_empty() {
        return Err(format!(
            "{}\nfix each, then run moor-machine-agent up again",
            rootless::report(&failed)
        ));
    }
    say(&rootless::notice(&facts.user));
    for w in rootless::warnings(&facts) {
        say(&format!(
            "warning: {}: {}; fix: {}",
            w.check, w.detail, w.fix
        ));
    }
    Ok(())
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
