#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::path::{Path, PathBuf};

use super::files::{controller, converge_user, Layout};
use super::gate::{self, Gate, Plan as GatePlan};
use super::node::{agent_unit, rootless_checks, runner_unit, Plan, Ports};
use super::relay::{relay_unit, Relay};
use super::service::{service_unit, socket_unit, Unit};
use super::wait_for;
use crate::env::Env;
use moor_client::rootless::Facts;

fn env() -> Env {
    Env::from_pairs([("HOME", "/home/ada")])
}

const SOCK: &str = "/home/ada/.local/state/moor/controller.sock";

/// The `moor_service` section `up` adds.
const MOOR_SERVICE: &str = "moor_service:\n  upstream: 127.0.0.1:8444\n";

fn converge(existing: Option<&str>) -> Result<Option<String>, String> {
    converge_user(&env(), "config.yaml", existing, Path::new(SOCK))
        .map_err(|c| format!("{}; fix: {}", c.detail, c.fix))
}

#[test]
fn a_missing_user_config_gets_the_controller_socket_and_the_gate_route() {
    let text = converge(None).unwrap().unwrap();
    assert_eq!(
        text,
        format!("controller:\n  socket: {SOCK}\n{MOOR_SERVICE}")
    );
    let config = moor_core::config::parse_user("x", &text).unwrap();
    assert_eq!(
        config.moor_service.unwrap().upstream.as_str(),
        "127.0.0.1:8444"
    );
}

#[test]
fn a_moor_service_of_the_users_own_is_kept() {
    let existing =
        format!("controller:\n  socket: {SOCK}\nmoor_service:\n  upstream: 10.0.0.9:8444\n");
    assert_eq!(converge(Some(&existing)).unwrap(), None);
}

#[test]
fn a_converged_user_config_needs_no_change() {
    let text = converge(None).unwrap().unwrap();
    assert_eq!(converge(Some(&text)).unwrap(), None);
}

#[test]
fn other_settings_and_comments_are_kept_when_the_controller_is_added() {
    let existing = "# mine\nbox:\n  sleep_after: 30m";
    let text = converge(Some(existing)).unwrap().unwrap();
    assert_eq!(
        text,
        format!("# mine\nbox:\n  sleep_after: 30m\ncontroller:\n  socket: {SOCK}\n{MOOR_SERVICE}")
    );
}

#[test]
fn a_home_relative_socket_counts_as_the_same_controller() {
    let existing =
        format!("controller:\n  socket: ~/.local/state/moor/controller.sock\n{MOOR_SERVICE}");
    assert_eq!(converge(Some(&existing)).unwrap(), None);
}

#[test]
fn a_config_naming_another_controller_says_what_to_set() {
    let err = converge(Some("controller:\n  socket: /elsewhere.sock\n")).unwrap_err();
    assert!(
        err.contains(&format!("set controller.socket to {SOCK}")),
        "{err}"
    );
}

#[test]
fn the_node_an_earlier_up_wrote_is_named_for_deletion() {
    let existing = format!(
        "controller:\n  socket: {SOCK}\nnodes:\n  local:\n    runtime: external\n    max_boxes: 9\n"
    );
    let err = converge(Some(&existing)).unwrap_err();
    assert!(err.contains("delete nodes.local from config.yaml"), "{err}");
}

#[test]
fn other_nodes_are_left_alone() {
    let existing = format!(
        "controller:\n  socket: {SOCK}\nnodes:\n  big:\n    runtime: static\n{MOOR_SERVICE}"
    );
    assert_eq!(converge(Some(&existing)).unwrap(), None);
}

#[test]
fn an_invalid_user_config_says_which_file_to_fix() {
    let err = converge(Some("nodes: [oops")).unwrap_err();
    assert!(err.contains("fix config.yaml"), "{err}");
}

fn unit() -> Unit {
    Unit {
        exe: PathBuf::from("/home/ada/.local/bin/moor"),
        config: PathBuf::from("/home/ada/.config/moor/controller.yaml"),
        socket: PathBuf::from(SOCK),
        env: vec![
            (
                "MOOR_CONFIG".to_owned(),
                "/home/ada/.config/moor/config.yaml".to_owned(),
            ),
            (
                "PATH".to_owned(),
                "/home/ada/.local/bin:/usr/bin".to_owned(),
            ),
        ],
        env_file: None,
    }
}

#[test]
fn the_service_runs_the_controller_with_the_installing_shells_path() {
    assert_eq!(
        service_unit(&unit()),
        "[Unit]
Description=moor controller
Requires=moor.socket
After=moor.socket

[Service]
ExecStart=\"/home/ada/.local/bin/moor\" \"controller\" \"serve\" \"--config\" \"/home/ada/.config/moor/controller.yaml\"
Environment=\"MOOR_CONFIG=/home/ada/.config/moor/config.yaml\"
Environment=\"PATH=/home/ada/.local/bin:/usr/bin\"
KillMode=process
TimeoutStopSec=660
Restart=on-failure

[Install]
WantedBy=default.target
"
    );
}

#[test]
fn the_socket_unit_holds_the_configured_controller_socket() {
    let text = socket_unit(&unit());
    assert!(text.contains(&format!("ListenStream={SOCK}\n")), "{text}");
    assert!(text.contains("FileDescriptorName=controller\n"), "{text}");
    assert!(text.contains("SocketMode=0600\n"), "{text}");
}

#[test]
fn a_percent_in_a_path_is_not_a_systemd_specifier() {
    let mut u = unit();
    u.exe = PathBuf::from("/opt/100%/moor");
    u.socket = PathBuf::from("/run/100%/controller.sock");
    assert!(service_unit(&u).contains("ExecStart=\"/opt/100%%/moor\""));
    assert!(socket_unit(&u).contains("ListenStream=/run/100%%/controller.sock"));
}

fn layout() -> Layout {
    Layout {
        controller_config: PathBuf::from("/home/ada/.config/moor/controller.yaml"),
        user_config: PathBuf::from("/home/ada/.config/moor/config.yaml"),
        state_dir: PathBuf::from("/home/ada/.local/state/moor"),
        socket: PathBuf::from(SOCK),
    }
}

fn plan() -> Plan {
    Plan {
        runner: PathBuf::from("/home/ada/.local/bin/moor-machine-runner"),
        agent: PathBuf::from("/home/ada/.local/bin/moor-machine-agent"),
        ports: Ports {
            agents: 20001,
            listen: 20002,
            egress: 20003,
        },
        egress: "172.17.0.1:20003".parse().unwrap(),
    }
}

#[test]
fn the_runner_takes_agent_links_on_loopback_and_the_controller_on_its_runner_socket() {
    assert_eq!(
        runner_unit(&plan(), &layout(), Some("/usr/bin")),
        "[Unit]
Description=moor machine runner: takes the local machine agent's link
Wants=moor.service
After=moor.service

[Service]
ExecStart=\"/home/ada/.local/bin/moor-machine-runner\" \"--socket\" \"/home/ada/.local/state/moor/runner/runner.sock\" \"--state-dir\" \"/home/ada/.local/state/moor/machine-runner\" \"--agents\" \"127.0.0.1:20001\" \"--listen\" \"127.0.0.1:20002\"
Environment=\"PATH=/usr/bin\"
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"
    );
}

#[test]
fn the_agent_runs_under_its_own_home_and_state_and_keeps_the_users_path_to_docker() {
    let env = Env::from_pairs([
        ("HOME", "/nonexistent-home"),
        ("PATH", "/usr/bin"),
        ("DOCKER_HOST", "unix:///run/user/1000/docker.sock"),
    ]);
    let text = agent_unit(&plan(), Path::new("/state/agent-home"), &env);
    assert!(
        text.contains("ExecStart=\"/home/ada/.local/bin/moor-machine-agent\" \"run\"\n"),
        "{text}"
    );
    for line in [
        "Environment=\"HOME=/state/agent-home\"",
        "Environment=\"XDG_STATE_HOME=/state/agent-home/.local/state\"",
        "Environment=\"PATH=/usr/bin\"",
        "Environment=\"DOCKER_HOST=unix:///run/user/1000/docker.sock\"",
        "After=moor-runner.service",
    ] {
        assert!(text.contains(line), "{line} missing from {text}");
    }
    assert!(!text.to_lowercase().contains("crabbox"));
}

fn gate_plan() -> GatePlan {
    GatePlan {
        exe: PathBuf::from("/home/ada/.local/bin/moor-gate"),
        key_file: PathBuf::from("/home/ada/.local/state/moor/service.env"),
        front_socket: PathBuf::from("/home/ada/.local/state/moor/fronts/authgear/front"),
    }
}

#[test]
fn the_gate_serves_only_boxes_and_takes_the_key_from_its_file() {
    assert_eq!(
        gate::unit(&gate_plan()),
        "[Unit]
Description=moor gate: serves report_step to the Boxes on this host
Wants=moor.service
After=moor.service

[Service]
ExecStart=\"/home/ada/.local/bin/moor-gate\" \"--listen\" \"off\" \"--box-listen\" \"127.0.0.1:8444\" \"--front-socket\" \"/home/ada/.local/state/moor/fronts/authgear/front\"
EnvironmentFile=/home/ada/.local/state/moor/service.env
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
"
    );
}

#[test]
fn the_controller_unit_reads_the_same_key_file() {
    let mut u = unit();
    u.env_file = Some(PathBuf::from("/home/ada/.local/state/moor/service.env"));
    assert!(service_unit(&u).contains(
        "Environment=\"PATH=/home/ada/.local/bin:/usr/bin\"\nEnvironmentFile=/home/ada/.local/state/moor/service.env\nKillMode=process"
    ));
}

#[test]
fn the_service_key_is_made_once_and_only_its_owner_reads_it() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = moor_testkit::short_tempdir("key").unwrap();
    let plan = GatePlan {
        key_file: dir.path().join("service.env"),
        ..gate_plan()
    };
    gate::keep_key(&plan).unwrap();
    let first = std::fs::read_to_string(&plan.key_file).unwrap();
    assert!(
        first.starts_with("MOOR_SERVICE_KEY=") && first.trim().len() > 40,
        "{first}"
    );
    assert_eq!(
        std::fs::metadata(&plan.key_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    gate::keep_key(&plan).unwrap();
    assert_eq!(std::fs::read_to_string(&plan.key_file).unwrap(), first);
    std::fs::write(&plan.key_file, "MOOR_SERVICE_KEY=\n").unwrap();
    gate::keep_key(&plan).unwrap();
    assert_ne!(
        std::fs::read_to_string(&plan.key_file).unwrap(),
        "MOOR_SERVICE_KEY=\n"
    );
}

#[test]
fn the_controller_config_up_writes_lets_a_gate_report_steps() {
    let dir = moor_testkit::short_tempdir("cfg").unwrap();
    let mut l = layout();
    l.controller_config = dir.path().join("controller.yaml");
    controller(&l, true).unwrap();
    let text = std::fs::read_to_string(&l.controller_config).unwrap();
    let config = moor_core::config::parse_controller("controller.yaml", &text).unwrap();
    assert_eq!(
        config.step_report_fronts().into_iter().collect::<Vec<_>>(),
        ["authgear"]
    );
    let before = text;
    controller(&l, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&l.controller_config).unwrap(),
        before
    );
}

#[test]
fn a_user_config_routing_moor_service_elsewhere_gets_no_gate() {
    let dir = moor_testkit::short_tempdir("own").unwrap();
    let mut l = layout();
    l.user_config = dir.path().join("config.yaml");
    l.controller_config = dir.path().join("controller.yaml");
    std::fs::write(&l.user_config, "moor_service:\n  upstream: 10.0.0.9:8444\n").unwrap();
    let got = gate::decide(&env(), &l, dir.path()).unwrap();
    assert_eq!(got, Gate::Left("moor_service names a listener of your own"));
}

fn relay() -> Relay {
    Relay {
        forward: PathBuf::from("/home/ada/.local/bin/moor-egress-forward"),
        state: PathBuf::from("/run/user/1000/dockerd-rootless"),
        docker: "docker.service".to_owned(),
    }
}

#[test]
fn the_relay_enters_the_daemons_namespace_at_start_and_follows_the_rootless_docker_unit() {
    let mut p = plan();
    p.egress = "10.0.2.100:20003".parse().unwrap();
    assert_eq!(
        relay_unit(&p, &layout(), &relay(), Some("/usr/bin")),
        "[Unit]
Description=moor egress relay: carries Box egress into the rootless Docker namespace
BindsTo=docker.service
PartOf=docker.service
After=docker.service

[Service]
ExecStart=\"/bin/sh\" \"-c\" \"exec nsenter -U --preserve-credentials -n -t $$(cat \\\"$$1\\\"/child_pid) \\\"$$2\\\" --relay \\\"$$3\\\" --socket \\\"$$4\\\"\" \"moor-relay\" \"/run/user/1000/dockerd-rootless\" \"/home/ada/.local/bin/moor-egress-forward\" \"10.0.2.100:20003\" \"/home/ada/.local/state/moor/egress/egress.sock\"
Environment=\"PATH=/usr/bin\"
Restart=always
RestartSec=2

[Install]
WantedBy=default.target docker.service
"
    );
}

#[test]
fn a_dollar_or_percent_in_a_relay_path_is_not_expanded_by_systemd() {
    let mut r = relay();
    r.state = PathBuf::from("/run/$u/100%/dockerd-rootless");
    let text = relay_unit(&plan(), &layout(), &r, None);
    assert!(
        text.contains("\"/run/$$u/100%%/dockerd-rootless\""),
        "{text}"
    );
}

#[test]
fn a_controller_unit_names_a_relayed_address_and_never_asks_to_bind_it() {
    let egress = "10.0.2.100:20003".parse().unwrap();
    let env = Env::from_pairs([("HOME", "/home/ada")]);
    let relayed = Unit::new(&env, &layout(), egress, true, None).unwrap();
    assert!(relayed.env.contains(&(
        "MOOR_EGRESS_RELAYED".to_owned(),
        "10.0.2.100:20003".to_owned()
    )));
    assert!(relayed.env.iter().all(|(k, _)| k != "MOOR_EGRESS_LISTEN"));
    let bound = Unit::new(&env, &layout(), egress, false, None).unwrap();
    assert!(bound.env.contains(&(
        "MOOR_EGRESS_LISTEN".to_owned(),
        "10.0.2.100:20003".to_owned()
    )));
    assert!(bound.env.iter().all(|(k, _)| k != "MOOR_EGRESS_RELAYED"));
}

/// A rootless host where every preflight check passes.
fn rootless_host() -> Facts {
    Facts {
        user: "ada".to_owned(),
        uid: 1000,
        subuid: Some("ada:100000:65536\n".to_owned()),
        subgid: Some("ada:100000:65536\n".to_owned()),
        missing: Vec::new(),
        apparmor_restrict: Some("0\n".to_owned()),
        unit: Some("enabled".to_owned()),
        linger: Some(true),
        rootlesskit: Ok(vec![
            "/proc/self/exe".to_owned(),
            "--net=slirp4netns".to_owned(),
            "--disable-host-loopback".to_owned(),
        ]),
        kvm_mode: Some(0o666),
    }
}

#[test]
fn up_stops_on_a_rootless_daemon_that_opens_the_hosts_loopback() {
    assert!(rootless_checks(&rootless_host()).is_ok());
    let mut open = rootless_host();
    open.rootlesskit = Ok(vec![
        "/proc/self/exe".to_owned(),
        "--net=slirp4netns".to_owned(),
    ]);
    let stopped = rootless_checks(&open).unwrap_err();
    assert!(
        stopped.detail.contains("--disable-host-loopback"),
        "{stopped:?}"
    );
    open.missing = vec!["nsenter".to_owned()];
    let stopped = rootless_checks(&open).unwrap_err();
    assert!(
        stopped.detail.contains("nsenter"),
        "the relay needs nsenter: {stopped:?}"
    );
}

#[test]
fn the_wait_fails_on_a_socket_that_takes_connections_but_never_answers() {
    let dir = moor_testkit::short_tempdir("held").unwrap();
    let socket = dir.path().join("controller.sock");
    let _held = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let failed = wait_for(&socket, std::time::Duration::from_millis(500)).unwrap_err();
    assert!(failed.detail.contains("no answer at"), "{failed:?}");
    assert!(
        failed.fix.contains("journalctl --user -u moor.service"),
        "{failed:?}"
    );
}
