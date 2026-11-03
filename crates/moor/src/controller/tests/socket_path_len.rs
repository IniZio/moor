//! A controller whose socket paths cannot fit a Unix socket address refuses to start.

use std::path::Path;

use super::{config, env, env_with};
use crate::controller::start;

/// A directory path of exactly `total` bytes under `base`.
fn deep(base: &Path, total: usize) -> String {
    let mut path = base.display().to_string();
    while path.len() < total {
        let room = total - path.len();
        path.push('/');
        path.extend(std::iter::repeat_n(
            'd',
            room.saturating_sub(1).clamp(1, 60),
        ));
    }
    path.truncate(total);
    path
}

fn refusal(env: &crate::env::Env, cfg: &Path) -> String {
    start(env, cfg)
        .err()
        .expect("a controller with an over-long socket path must not start")
        .message
}

#[test]
fn a_long_state_dir_is_refused_with_its_length_and_the_limit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let long = deep(tmp.path(), 150);
    let env = env_with(tmp.path(), &[("MOOR_STATE_DIR", &long)]);
    let msg = refusal(&env, &config(tmp.path(), ""));
    assert!(msg.contains("too long for a Unix socket"), "{msg}");
    assert!(msg.contains(&long), "{msg}");
    assert!(msg.contains("bytes, the limit is"), "{msg}");
    assert!(msg.contains("state_dir"), "{msg}");
    assert!(!Path::new(&long).exists(), "nothing is created first");
}

#[test]
fn a_long_configured_socket_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let long = format!("{}/c.sock", deep(tmp.path(), 150));
    let cfg = config(tmp.path(), &format!("socket: {long}\n"));
    let msg = refusal(&env(tmp.path()), &cfg);
    assert!(msg.contains("too long for a Unix socket"), "{msg}");
    assert!(msg.contains(&long), "{msg}");
    assert!(msg.contains("`socket`"), "{msg}");
}

#[test]
fn a_state_dir_whose_runner_socket_is_too_long_is_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = deep(tmp.path(), super::super::socket_paths::LIMIT - 17);
    let env = env_with(tmp.path(), &[("MOOR_STATE_DIR", &state)]);
    let cfg = config(tmp.path(), "socket: /tmp/moor-short-ok.sock\n");
    let msg = refusal(&env, &cfg);
    assert!(msg.contains("too long for a Unix socket"), "{msg}");
    assert!(msg.contains("runner.sock"), "{msg}");
}
