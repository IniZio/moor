#![allow(clippy::unwrap_used, reason = "test code")]

use std::path::PathBuf;

use super::Layout;
use crate::env::Env;

fn layout() -> Layout {
    Layout {
        controller_config: PathBuf::from("/home/ada/.config/moor/controller.yaml"),
        user_config: PathBuf::from("/home/ada/.config/moor/config.yaml"),
        state_dir: PathBuf::from("/home/ada/.local/state/moor"),
        socket: PathBuf::from("/home/ada/.local/state/moor/controller.sock"),
    }
}

#[test]
fn a_state_dir_too_long_for_the_runners_forward_socket_is_named_with_its_length_and_the_limit() {
    let fits = layout();
    assert_eq!(fits.socket_too_long(), None);
    let limit = crate::controller::socket_paths::LIMIT;
    let state = format!(
        "/{}",
        "s".repeat(limit - "/runner/machine-forwards.sock".len())
    );
    let long = Layout {
        state_dir: PathBuf::from(&state),
        socket: PathBuf::from("/c.sock"),
        ..layout()
    };
    let why = long.socket_too_long().unwrap();
    assert!(
        why.starts_with(&format!(
            "socket path {state}/runner/machine-forwards.sock is too long for a Unix socket: {} bytes, the limit is {limit}",
            limit + 1
        )),
        "{why}"
    );
}

#[test]
fn reading_the_layout_of_a_state_dir_too_long_for_its_sockets_refuses() {
    let home = tempfile::tempdir().unwrap();
    let state = format!("/{}", "s".repeat(100));
    let env = Env::from_pairs([
        ("HOME", home.path().to_str().unwrap()),
        ("MOOR_STATE_DIR", state.as_str()),
    ]);
    let refused = Layout::read(&env).unwrap_err();
    assert!(
        refused
            .detail
            .starts_with(&format!("socket path {state}/controller.sock is too long")),
        "{refused:?}"
    );
}
