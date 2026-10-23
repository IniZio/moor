//! A front's `read_only` paths may not show it the controller's own files.

use super::{config, env};
use crate::controller::start;

/// The refusal `start` gives for a front that may read `paths`, with its config in `cfg/` and
/// its socket in `sock/`.
fn refusal(paths: &[&str]) -> Option<String> {
    refusal_with_state(paths, "ROOT/state")
}

/// Like [`refusal`], with the controller's state dir at `state`.
fn refusal_with_state(paths: &[&str], state: &str) -> Option<String> {
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::create_dir(dir.path().join("sock")).expect("sock dir");
    let root = dir.path().display().to_string();
    let paths: Vec<String> = paths.iter().map(|p| p.replace("ROOT", &root)).collect();
    std::fs::create_dir(dir.path().join("cfg")).expect("cfg dir");
    let cfg = config(
        &dir.path().join("cfg"),
        &format!(
            "socket: {root}/sock/c.sock\nstate_dir: {}\nfronts:\n  web:\n    command: [\"true\"]\n    read_only: {}\n",
            state.replace("ROOT", &root),
            serde_json::to_string(&paths).expect("json")
        ),
    );
    start(&env(dir.path()), &cfg).err().map(|stop| stop.message)
}

#[test]
fn a_read_only_path_inside_the_controller_state_dir_is_refused() {
    let e = refusal(&["ROOT/state/fronts"]).expect("refused");
    assert!(e.contains("fronts.web.read_only"), "{e}");
    assert!(e.contains("would show the controller's"), "{e}");
}

#[test]
fn a_read_only_path_holding_the_config_dir_vault_key_or_socket_is_refused() {
    for path in ["ROOT", "ROOT/cfg", "ROOT/cfg/vault.key", "ROOT/sock"] {
        let e = refusal(&[path]).expect("refused");
        assert!(e.contains("would show the controller's"), "{path}: {e}");
    }
}

#[test]
fn a_read_only_path_elsewhere_is_accepted() {
    assert_eq!(refusal(&["/usr/share"]), None);
}

#[test]
fn a_read_only_path_that_is_not_real_is_refused() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink("/usr/share", &link).expect("link");
    let link = link.display().to_string();
    for path in [link.as_str(), "/usr/share/../share"] {
        let e = refusal(&[path]).expect("refused");
        assert!(e.contains("is not a real path"), "{path}: {e}");
    }
}

#[test]
fn a_read_only_parent_of_the_state_dir_is_refused() {
    let e = refusal_with_state(&["ROOT/deep"], "ROOT/deep/state").expect("refused");
    assert!(e.contains("would show the controller's"), "{e}");
}
