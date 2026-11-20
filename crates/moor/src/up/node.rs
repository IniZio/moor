//! The local node `up` makes: a machine agent on this host that provides Docker Boxes, the
//! machine runner that takes its link on loopback, and the node the controller holds for it.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash as _, Hasher as _};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use moor_core::request::AsReq;
use moor_core::view::node::{NodeAddValue, NodeListValue};
use moor_core::Request;
use serde::{Deserialize, Serialize};

use super::files::{Layout, NODE};
use super::service;
use moor_client::rootless;

use crate::doctor::{answer, Ask, Check};
use crate::env::Env;
use crate::node_add::{self, Add};

/// The machine runner's executable.
const RUNNER_BIN: &str = "moor-machine-runner";
/// The machine agent's executable.
const AGENT_BIN: &str = "moor-machine-agent";
/// How to get both when they are missing.
pub(super) const INSTALL: &str =
    "run `curl -fsSL https://github.com/IniZio/moor/releases/latest/download/install.sh | sh`";
/// What `up` tells the person about the privileges Docker Boxes take.
const PRIVILEGED: &str = "Docker Boxes are privileged containers: each Box's work place runs with \
    --privileged so Docker works inside it, and a user who can run Docker is root on this host. \
    Only your own Boxes run here";
/// What `up` tells the person about the images of a rootless daemon.
const IMAGES: &str = "the rootless daemon keeps its own images and cache volumes under your home, \
    so the first Box here pulls the Box images again";
/// The rootless Docker daemon's own user unit.
const DOCKER_UNIT: &str = "docker.service";
/// The file in the state dir that keeps the ports `up` chose.
const PORTS_FILE: &str = "up-ports.json";
/// Seconds to wait for the agent's link; `MOOR_UP_LINK_WAIT_SECS` sets another.
const LINK_WAIT_SECS: u64 = 60;
/// Pause between looks at the node list while waiting.
const LINK_POLL: Duration = Duration::from_millis(500);

/// The ports `up` chose once and keeps: the runner takes the agent's link and serves its own
/// tools on loopback, and the controller's Proxy listens on the docker0 address for Boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ports {
    /// The runner's `--agents` port.
    pub agents: u16,
    /// The runner's `--listen` port.
    pub listen: u16,
    /// The controller's `MOOR_EGRESS_LISTEN` port.
    pub egress: u16,
}

impl Ports {
    /// The ports kept in `state_dir`, else the first free ones from a base that the state dir
    /// picks, written there.
    fn read_or_pick(state_dir: &Path, gateway: Ipv4Addr) -> Result<Self, Check> {
        let path = state_dir.join(PORTS_FILE);
        if let Some(kept) = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            return Ok(kept);
        }
        let mut hash = DefaultHasher::new();
        state_dir.hash(&mut hash);
        let mut next = 20_000 + u16::try_from(hash.finish() % 9_000).unwrap_or(0);
        let mut take = |ip: IpAddr| -> Result<u16, Check> {
            let port = (next..next.saturating_add(500))
                .find(|p| TcpListener::bind((ip, *p)).is_ok())
                .ok_or_else(|| {
                    Check::fail("node", format!("no free port near {next} on {ip}"), "")
                })?;
            next = port + 1;
            Ok(port)
        };
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let ports = Self {
            agents: take(loopback)?,
            listen: take(loopback)?,
            egress: take(IpAddr::V4(gateway))?,
        };
        let text = serde_json::to_string(&ports).unwrap_or_default();
        super::files::write(&path, &format!("{text}\n"))?;
        Ok(ports)
    }
}

/// What the node's units need, found before `up` writes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The machine runner's executable.
    pub runner: PathBuf,
    /// The machine agent's executable.
    pub agent: PathBuf,
    /// The chosen ports.
    pub ports: Ports,
    /// The docker0 address and the port the controller's Proxy also listens on.
    pub egress: SocketAddr,
}

/// Checks that Docker works for this user and that both executables are installed, says what
/// Docker Boxes are, and chooses the ports.
///
/// # Errors
/// Docker is not usable, an executable is missing, or no port is free.
pub(crate) fn plan(env: &Env, layout: &Layout) -> Result<(Plan, Option<Relay>, Vec<Check>), Check> {
    let version = docker(&["info", "--format", "{{.ServerVersion}}"]).map_err(|why| {
        Check::fail(
            "docker",
            format!("Docker does not work for this user: {why}"),
            "install Docker and let this user run it, such as by joining the docker group, then rerun moor up",
        )
    })?;
    let rootless = rootless::detect();
    let notes = trust(env, rootless)?;
    let (host, relay) = if rootless {
        let (ip, relay) = relayed(env)?;
        (ip, Some(relay))
    } else {
        (bridge_gateway()?, None)
    };
    let missing = |bin: &str| {
        Check::fail(
            "node",
            format!("{bin} is neither beside moor nor on PATH"),
            INSTALL,
        )
    };
    let runner = binary(env, RUNNER_BIN).ok_or_else(|| missing(RUNNER_BIN))?;
    let agent = binary(env, AGENT_BIN).ok_or_else(|| missing(AGENT_BIN))?;
    // The relay's namespace is not this host's, so a port free on loopback stands in for one free there.
    let probe = if relay.is_some() {
        Ipv4Addr::LOCALHOST
    } else {
        host
    };
    let ports = Ports::read_or_pick(&layout.state_dir, probe)?;
    let mut checks = vec![Check::ok("docker", format!("Docker {version} answers"))];
    checks.extend(notes);
    let plan = Plan {
        runner,
        agent,
        ports,
        egress: SocketAddr::new(IpAddr::V4(host), ports.egress),
    };
    Ok((plan, relay, checks))
}

/// The address of Docker's default bridge on this host.
fn bridge_gateway() -> Result<Ipv4Addr, Check> {
    docker(&[
        "network",
        "inspect",
        "bridge",
        "--format",
        "{{(index .IPAM.Config 0).Gateway}}",
    ])
    .ok()
    .and_then(|g| g.parse::<Ipv4Addr>().ok())
    .ok_or_else(|| {
        Check::fail(
            "docker",
            "cannot read the address of Docker's bridge network",
            "check `docker network inspect bridge`",
        )
    })
}

/// What the egress relay of a rootless daemon needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relay {
    /// The `moor-egress-forward` executable, which has a relay mode.
    pub forward: PathBuf,
    /// The state dir of the daemon's rootlesskit, which holds `child_pid`.
    pub state: PathBuf,
    /// The user unit that runs the daemon.
    pub docker: String,
}

/// The rootless daemon's namespace address, which the relay listens on and a Box dials, and the
/// relay's own facts.
fn relayed(env: &Env) -> Result<(Ipv4Addr, Relay), Check> {
    let fail = |why: String| Check::fail("relay", why, "check that the rootless daemon is running");
    let state = rootless::daemon_state().map_err(fail)?;
    let ip = rootless::child_ip(&state).map_err(fail)?;
    let forward = crate::proxy::route::binary(env);
    if !fs::metadata(&forward).is_ok_and(|m| m.is_file()) {
        return Err(Check::fail(
            "relay",
            format!("{} does not exist", forward.display()),
            INSTALL,
        ));
    }
    let docker = DOCKER_UNIT.to_owned();
    Ok((
        ip,
        Relay {
            forward,
            state,
            docker,
        },
    ))
}

/// What `up` says about the privileges Docker Boxes take. Under rootless Docker it first checks
/// the daemon is set up for them, and fails with every check that does not hold.
fn trust(env: &Env, rootless: bool) -> Result<Vec<Check>, Check> {
    if !rootless {
        return Ok(vec![Check::ok("docker", PRIVILEGED)]);
    }
    rootless_checks(&rootless::Facts::read(env.get("PATH"), env.get("USER")))
}

/// What `up` says under rootless Docker for the host `facts` describe, or every check that
/// fails, with an open host loopback and a missing `nsenter` among them.
pub(crate) fn rootless_checks(facts: &rootless::Facts) -> Result<Vec<Check>, Check> {
    let failed = rootless::failures(facts, true);
    if !failed.is_empty() {
        return Err(Check::fail("docker", rootless::report(&failed), ""));
    }
    let mut checks = vec![
        Check::ok("docker", rootless::notice(&facts.user)),
        Check::ok("docker", IMAGES),
    ];
    checks.extend(
        rootless::warnings(facts)
            .into_iter()
            .map(|w| Check::warn(w.check, w.detail, w.fix)),
    );
    Ok(checks)
}

/// The trimmed stdout of `docker args`, else why it failed.
fn docker(args: &[&str]) -> Result<String, String> {
    let out = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
}

/// `name` beside this executable, else the first on `PATH`.
pub(super) fn binary(env: &Env, name: &str) -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(name)));
    let on_path = env
        .get("PATH")
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join(name));
    beside
        .into_iter()
        .chain(on_path)
        .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file()))
}

/// The runner's unit text.
pub(crate) fn runner_unit(plan: &Plan, layout: &Layout, path: Option<&str>) -> String {
    let args: Vec<String> = [
        ("--socket", layout.runner_socket().display().to_string()),
        ("--state-dir", layout.runner_state().display().to_string()),
        ("--agents", format!("127.0.0.1:{}", plan.ports.agents)),
        ("--listen", format!("127.0.0.1:{}", plan.ports.listen)),
    ]
    .into_iter()
    .flat_map(|(flag, value)| [flag.to_owned(), value])
    .collect();
    let env: Vec<_> = path
        .map(|p| ("PATH".to_owned(), p.to_owned()))
        .into_iter()
        .collect();
    service::process_unit(
        "moor machine runner: takes the local machine agent's link",
        service::SERVICE,
        &plan.runner,
        &args,
        &env,
        None,
    )
}

/// The agent's unit text. It runs as `HOME=<agent home>`, where `moor node add` leaves its key
/// and state; Docker is reached the way this user reaches it.
pub(crate) fn agent_unit(plan: &Plan, home: &Path, env: &Env) -> String {
    let state = home.join(".local/state");
    let mut vars = vec![
        ("HOME".to_owned(), home.display().to_string()),
        ("XDG_STATE_HOME".to_owned(), state.display().to_string()),
    ];
    for name in ["PATH", "DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG"] {
        if let Some(v) = env.get(name).filter(|v| !v.is_empty()) {
            vars.push((name.to_owned(), v.to_owned()));
        }
    }
    if env.get("DOCKER_CONFIG").is_none_or(str::is_empty) {
        if let Some(own) = env.get("HOME").map(|h| Path::new(h).join(".docker")) {
            if own.is_dir() {
                vars.push(("DOCKER_CONFIG".to_owned(), own.display().to_string()));
            }
        }
    }
    service::process_unit(
        "moor machine agent: provides Docker Boxes on this host",
        service::RUNNER,
        &plan.agent,
        &["run".to_owned()],
        &vars,
        None,
    )
}

/// The relay's unit text: it enters the rootless daemon's namespace as it is when the unit starts,
/// and follows the daemon's own unit, so a new namespace gets a new relay.
pub(crate) fn relay_unit(
    plan: &Plan,
    layout: &Layout,
    relay: &Relay,
    path: Option<&str>,
) -> String {
    let script = "exec nsenter -U --preserve-credentials -n -t $(cat \"$1\"/child_pid) \"$2\" --relay \"$3\" --socket \"$4\"";
    let words = [
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
        "moor-relay".to_owned(),
        relay.state.display().to_string(),
        relay.forward.display().to_string(),
        plan.egress.to_string(),
        crate::proxy::route::local_socket(&layout.state_dir)
            .display()
            .to_string(),
    ];
    let exec = words
        .iter()
        .map(|w| service::quoted(&w.replace('$', "$$")))
        .collect::<Vec<_>>()
        .join(" ");
    let vars = path.map_or_else(String::new, |p| {
        format!("Environment={}\n", service::quoted(&format!("PATH={p}")))
    });
    let docker = &relay.docker;
    format!(
        "[Unit]
Description=moor egress relay: carries Box egress into the rootless Docker namespace
BindsTo={docker}
PartOf={docker}
After={docker}

[Service]
ExecStart={exec}
{vars}Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"
    )
}

/// Installs the runner, adds the node, installs the agent and waits for its link; one check
/// for each.
///
/// # Errors
/// A unit cannot be installed or the controller refuses the node.
pub(crate) fn converge(
    env: &Env,
    ask: Ask<'_>,
    layout: &Layout,
    plan: &Plan,
    relay: Option<&Relay>,
    dir: &Path,
) -> Result<Vec<Check>, Check> {
    let path = env.get("PATH");
    let mut checks = Vec::new();
    if let Some(relay) = relay {
        let text = relay_unit(plan, layout, relay, path);
        service::install_one(dir, service::RELAY, &text, false)?;
        checks.push(Check::ok(
            "relay",
            format!(
                "{} relays Box egress on {} inside the rootless Docker namespace",
                service::RELAY,
                plan.egress
            ),
        ));
    }
    service::install_one(
        dir,
        service::RUNNER,
        &runner_unit(plan, layout, path),
        false,
    )?;
    checks.push(Check::ok(
        "runner",
        format!(
            "{} takes agent links on 127.0.0.1:{}",
            service::RUNNER,
            plan.ports.agents
        ),
    ));
    let add = Add {
        name: NODE.to_owned(),
        agent_home: layout.agent_home(),
        runner: format!("127.0.0.1:{}", plan.ports.agents),
        labels: Vec::new(),
    };
    let added = add_node(ask, &add)?;
    let unit = agent_unit(plan, &add.agent_home, env);
    service::install_one(dir, service::AGENT, &unit, added)?;
    checks.push(Check::ok(
        "node",
        format!(
            "{NODE}: Docker Boxes by {} with HOME {}",
            service::AGENT,
            add.agent_home.display()
        ),
    ));
    checks.push(linked(env, ask));
    Ok(checks)
}

/// Adds the node unless the controller already holds it and the agent's home already holds the
/// state that pairs it. Whether it was added.
fn add_node(ask: Ask<'_>, add: &Add) -> Result<bool, Check> {
    let fail = |why: String| Check::fail("node", why, "");
    let listed = answer::<NodeListValue>(ask, Request::NodeList(AsReq { identity: None }))
        .map_err(|p| Check::failed("node", p))?;
    let held = listed.nodes.iter().any(|n| n.name == add.name);
    let dir = node_add::agent_dir(&add.agent_home);
    let want = node_add::runner_url(&add.runner).map_err(fail)?;
    let paired = fs::read_to_string(dir.join("state.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|s| s.get("server").and_then(|v| v.as_str()) == Some(want.as_str()))
        && dir.join("id_ed25519").is_file();
    if held && paired {
        return Ok(false);
    }
    let req = node_add::prepare(add).map_err(fail)?;
    let value =
        answer::<NodeAddValue>(ask, Request::NodeAdd(req)).map_err(|p| Check::failed("node", p))?;
    node_add::finish(add, &value).map_err(fail)?;
    Ok(true)
}

/// Waits for the node's agent to link, then names how it provides Boxes.
fn linked(env: &Env, ask: Ask<'_>) -> Check {
    let secs = env
        .get("MOOR_UP_LINK_WAIT_SECS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(LINK_WAIT_SECS);
    let polls = secs * 1000 / u64::try_from(LINK_POLL.as_millis()).unwrap_or(500);
    for _ in 0..=polls {
        let listed = answer::<NodeListValue>(ask, Request::NodeList(AsReq { identity: None }));
        let node = listed
            .ok()
            .and_then(|l| l.nodes.into_iter().find(|n| n.name == NODE && n.online));
        if let Some(node) = node {
            let how = node.provides.unwrap_or(node.runtime);
            return Check::ok("node", format!("{NODE} is online, provides {how}"));
        }
        std::thread::sleep(LINK_POLL);
    }
    Check::warn(
        "node",
        format!("{NODE} has not linked yet"),
        format!(
            "read `journalctl --user -u {} -u {}`",
            service::AGENT,
            service::RUNNER
        ),
    )
}
