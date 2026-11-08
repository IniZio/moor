#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use super::files::{converge_user, max_boxes, Node};
use super::provider::{parse, Report};
use super::service::{service_unit, socket_unit, Unit};
use crate::env::Env;

fn node(asked: Option<u32>) -> Node {
    Node {
        runtime: "external".to_owned(),
        max_boxes: NonZeroU32::new(asked.unwrap_or(3)).unwrap(),
        asked: asked.is_some(),
    }
}

fn env() -> Env {
    Env::from_pairs([("HOME", "/home/ada")])
}

const SOCK: &str = "/home/ada/.local/state/moor/controller.sock";

fn converge(existing: Option<&str>, n: &Node) -> Result<Option<String>, String> {
    converge_user(&env(), "config.yaml", existing, Path::new(SOCK), n)
        .map_err(|c| format!("{}; fix: {}", c.detail, c.fix))
}

#[test]
fn a_missing_user_config_gets_the_controller_and_one_local_node() {
    let text = converge(None, &node(None)).unwrap().unwrap();
    assert_eq!(
        text,
        format!(
            "controller:\n  socket: {SOCK}\nnodes:\n  local:\n    runtime: external\n    max_boxes: 3\n"
        )
    );
    let config = moor_core::config::parse_user("x", &text).unwrap();
    assert_eq!(config.default_node(), Some("local"));
}

#[test]
fn a_converged_user_config_needs_no_change() {
    let text = converge(None, &node(None)).unwrap().unwrap();
    assert_eq!(converge(Some(&text), &node(None)).unwrap(), None);
}

#[test]
fn other_settings_and_comments_are_kept_when_sections_are_added() {
    let existing = "# mine\nbox:\n  sleep_after: 30m";
    let text = converge(Some(existing), &node(None)).unwrap().unwrap();
    assert!(
        text.starts_with("# mine\nbox:\n  sleep_after: 30m\ncontroller:\n"),
        "{text}"
    );
}

#[test]
fn a_home_relative_socket_counts_as_the_same_controller() {
    let existing = "controller:\n  socket: ~/.local/state/moor/controller.sock\n";
    let text = converge(Some(existing), &node(None)).unwrap().unwrap();
    assert!(text.starts_with(existing), "{text}");
    assert!(text.contains("nodes:\n  local:"), "{text}");
}

#[test]
fn a_config_naming_another_controller_says_what_to_set() {
    let err = converge(
        Some("controller:\n  socket: /elsewhere.sock\n"),
        &node(None),
    )
    .unwrap_err();
    assert!(
        err.contains(&format!("set controller.socket to {SOCK}")),
        "{err}"
    );
}

#[test]
fn existing_nodes_are_kept_without_a_max_boxes_flag() {
    let existing = format!(
        "controller:\n  socket: {SOCK}\nnodes:\n  local:\n    runtime: external\n    max_boxes: 9\n"
    );
    assert_eq!(converge(Some(&existing), &node(None)).unwrap(), None);
}

#[test]
fn a_max_boxes_flag_that_differs_from_the_file_names_the_key_to_edit() {
    let existing = format!(
        "controller:\n  socket: {SOCK}\nnodes:\n  local:\n    runtime: external\n    max_boxes: 9\n"
    );
    let err = converge(Some(&existing), &node(Some(2))).unwrap_err();
    assert!(err.contains("set nodes.local.max_boxes to 2"), "{err}");
}

#[test]
fn an_invalid_user_config_says_which_file_to_fix() {
    let err = converge(Some("nodes: [oops"), &node(None)).unwrap_err();
    assert!(err.contains("fix config.yaml"), "{err}");
}

#[test]
fn max_boxes_counts_four_gib_per_box_after_four_for_the_host() {
    assert_eq!(max_boxes(0).get(), 1);
    assert_eq!(max_boxes(6 * 1024).get(), 1);
    assert_eq!(max_boxes(16 * 1024).get(), 3);
    assert_eq!(max_boxes(64 * 1024).get(), 15);
}

#[test]
fn crabbox_doctor_reports_name_the_provider_and_each_failed_check() {
    let out = "adapter noise\n{\"ok\":false,\"provider\":\"external\",\"checks\":[\
        {\"status\":\"ok\",\"check\":\"config\"},\
        {\"status\":\"failed\",\"check\":\"kvm\",\"details\":{\"hint\":\"join_kvm_group\"}},\
        {\"status\":\"failed\",\"check\":\"passt\"}]}\n";
    assert_eq!(
        parse(out),
        Some(Report {
            provider: "external".to_owned(),
            failed: vec!["kvm (join_kvm_group)".to_owned(), "passt".to_owned()],
        })
    );
    assert_eq!(parse("not json"), None);
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
