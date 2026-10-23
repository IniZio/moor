//! A front's `read_only` paths may not show it the controller's own files.

use super::{config, env};
use crate::controller::start;

/// The refusal `start` gives for a front that may read `paths`, with its socket in `sock/`.
fn refusal(paths: &[&str]) -> Option<String> {
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::create_dir(dir.path().join("sock")).expect("sock dir");
    let root = dir.path().display().to_string();
    let paths: Vec<String> = paths.iter().map(|p| p.replace("ROOT", &root)).collect();
    let cfg = config(
        dir.path(),
        &format!(
            "socket: {root}/sock/c.sock\nfronts:\n  web:\n    command: [\"true\"]\n    read_only: {}\n",
            serde_json::to_string(&paths).expect("json")
        ),
    );
    start(&env(dir.path()), &cfg).err().map(|stop| stop.message)
}

#[test]
fn a_read_only_path_inside_the_controller_state_dir_is_refused() {
    let e = refusal(&["ROOT/state/fronts"]).expect("refused");
    assert!(e.contains("fronts.web.read_only"), "{e}");
    assert!(e.contains("inside the controller's state dir"), "{e}");
}

#[test]
fn a_read_only_path_holding_the_vault_key_or_the_socket_is_refused() {
    for path in ["ROOT", "ROOT/vault.key", "ROOT/sock"] {
        let e = refusal(&[path]).expect("refused");
        assert!(
            e.contains("holds the controller socket or vault key"),
            "{path}: {e}"
        );
    }
}

#[test]
fn a_read_only_path_elsewhere_is_accepted() {
    assert_eq!(refusal(&["/usr/share"]), None);
}
