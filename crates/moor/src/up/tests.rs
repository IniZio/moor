#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::path::{Path, PathBuf};

use super::files::{converge_user, Layout};
use super::node::{agent_unit, runner_unit, Plan, Ports};
use super::service::{service_unit, socket_unit, Unit};
use crate::env::Env;

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
