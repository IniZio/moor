#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::path::{Path, PathBuf};

use super::files::converge_user;
use super::service::{service_unit, socket_unit, Unit};
use crate::env::Env;

fn env() -> Env {
    Env::from_pairs([("HOME", "/home/ada")])
}

const SOCK: &str = "/home/ada/.local/state/moor/controller.sock";

fn converge(existing: Option<&str>) -> Result<Option<String>, String> {
    converge_user(&env(), "config.yaml", existing, Path::new(SOCK))
        .map_err(|c| format!("{}; fix: {}", c.detail, c.fix))
}

#[test]
fn a_missing_user_config_gets_the_controller_socket() {
    let text = converge(None).unwrap().unwrap();
    assert_eq!(text, format!("controller:\n  socket: {SOCK}\n"));
    moor_core::config::parse_user("x", &text).unwrap();
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
        format!("# mine\nbox:\n  sleep_after: 30m\ncontroller:\n  socket: {SOCK}\n")
    );
}

#[test]
fn a_home_relative_socket_counts_as_the_same_controller() {
    let existing = "controller:\n  socket: ~/.local/state/moor/controller.sock\n";
    assert_eq!(converge(Some(existing)).unwrap(), None);
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
    let existing = format!("controller:\n  socket: {SOCK}\nnodes:\n  big:\n    runtime: static\n");
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
