//! Tests for the credential files, keyed by agent and Slack user, and the per-user config
//! dirs.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use super::{config_dir, private_dir, AgentKind, SlackUser};

const CLAUDE: AgentKind = AgentKind::Claude;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn user(id: &str) -> SlackUser {
    SlackUser::parse(id).unwrap()
}

#[test]
fn only_slack_shaped_ids_name_a_file() {
    for good in ["U1", "W0ABC123", "U0123456789ABCDEFGHIJKLMNOPQRSTU"] {
        assert!(SlackUser::parse(good).is_some(), "{good}");
    }
    let too_long = format!("U{}", "A".repeat(32));
    for bad in [
        "", "U", "u1", "X1", "U1/..", "../U1", "U-1", "U 1", "U1\n", &too_long,
    ] {
        assert!(SlackUser::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn the_agent_is_named_by_one_word() {
    assert_eq!(AgentKind::Claude.name(), "claude");
}

#[test]
fn each_user_gets_a_private_config_dir_per_agent() {
    let state = tempfile::tempdir().unwrap();
    let dir = config_dir(state.path(), CLAUDE, &user("U1")).unwrap();
    assert_eq!(dir, state.path().join("claude/U1"));
    assert_eq!(mode(&dir), 0o700);
    assert_ne!(config_dir(state.path(), CLAUDE, &user("U2")).unwrap(), dir);
}

/// A directory outside the state dir, mode 0755, for a planted symlink to point at.
fn elsewhere() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

#[test]
fn a_symlinked_config_dir_is_refused_and_left_alone() {
    for link in ["claude", "claude/U1"] {
        let state = tempfile::tempdir().unwrap();
        let target = elsewhere();
        std::fs::create_dir(state.path().join("claude")).unwrap();
        let link = state.path().join(link);
        let _ = std::fs::remove_dir(&link);
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        let err = config_dir(state.path(), CLAUDE, &user("U1")).unwrap_err();
        assert!(err.contains("not a directory that this user owns"), "{err}");
        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
        assert_eq!(mode(target.path()), 0o755);
    }
}

#[test]
fn a_dir_another_user_owns_is_refused_and_left_alone() {
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join("credentials");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let me = rustix::process::geteuid().as_raw();
    let err = private_dir(&dir, me.checked_add(1).unwrap()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(mode(&dir), 0o755);
    private_dir(&dir, me).unwrap();
    assert_eq!(mode(&dir), 0o700);
}
