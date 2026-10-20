//! Tests for the credential files, keyed by agent and Slack user, and the per-user config
//! dirs.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use moor_core::secret::Secret;

use super::{config_dir, credential_path, private_dir, remove, store, AgentKind, SlackUser};

const CLAUDE: AgentKind = AgentKind::Claude;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn user(id: &str) -> SlackUser {
    SlackUser::parse(id).unwrap()
}

fn secret(text: &str) -> Secret {
    Secret::new(text.into())
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
fn agents_are_named_by_one_word() {
    assert_eq!(AgentKind::parse("claude"), Some(AgentKind::Claude));
    assert_eq!(AgentKind::DEFAULT.name(), "claude");
    for bad in ["", "Claude", "../claude", "codex"] {
        assert_eq!(AgentKind::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_stored_credential_is_keyed_by_agent_and_user_and_replaced_whole() {
    let state = tempfile::tempdir().unwrap();
    let u1 = user("U1");
    store(state.path(), CLAUDE, &u1, &secret("first-token")).unwrap();
    let path = credential_path(state.path(), CLAUDE, &u1);
    assert_eq!(path, state.path().join("credentials/claude/U1"));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&state.path().join("credentials")), 0o700);
    assert_eq!(mode(&state.path().join("credentials/claude")), 0o700);
    store(state.path(), CLAUDE, &u1, &secret("second")).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    let names: Vec<_> = std::fs::read_dir(state.path().join("credentials/claude"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["U1"], "no temp file is left behind");
}

#[test]
fn a_wider_credential_dir_is_narrowed_to_0700() {
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join("credentials/claude");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    store(state.path(), CLAUDE, &user("U1"), &secret("t")).unwrap();
    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn each_user_has_their_own_file_and_remove_deletes_only_theirs() {
    let state = tempfile::tempdir().unwrap();
    store(state.path(), CLAUDE, &user("U1"), &secret("one")).unwrap();
    store(state.path(), CLAUDE, &user("U2"), &secret("two")).unwrap();
    remove(state.path(), CLAUDE, &user("U1")).unwrap();
    remove(state.path(), CLAUDE, &user("U1")).unwrap();
    assert!(!credential_path(state.path(), CLAUDE, &user("U1")).exists());
    let kept = credential_path(state.path(), CLAUDE, &user("U2"));
    assert_eq!(std::fs::read(kept).unwrap(), b"two");
}

#[test]
fn a_failed_store_names_the_path_and_never_the_credential() {
    let state = tempfile::tempdir().unwrap();
    std::fs::write(state.path().join("credentials"), "a file, not a dir").unwrap();
    let err = store(state.path(), CLAUDE, &user("U1"), &secret("tok-xyz")).unwrap_err();
    assert!(err.contains("credentials"), "{err}");
    assert!(!err.contains("tok-xyz"), "{err}");
}

#[test]
fn a_failed_store_keeps_the_old_credential() {
    let state = tempfile::tempdir().unwrap();
    let u1 = user("U1");
    store(state.path(), CLAUDE, &u1, &secret("old-token")).unwrap();
    let blocker = state.path().join("credentials/claude/.U1.tmp");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("x"), "").unwrap();
    assert!(store(state.path(), CLAUDE, &u1, &secret("new-token")).is_err());
    let path = credential_path(state.path(), CLAUDE, &u1);
    assert_eq!(std::fs::read(path).unwrap(), b"old-token");
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
fn a_symlinked_credential_dir_is_refused_and_nothing_goes_through_it() {
    for link in ["credentials", "credentials/claude"] {
        let state = tempfile::tempdir().unwrap();
        let target = elsewhere();
        std::fs::create_dir(state.path().join("credentials")).unwrap();
        let link = state.path().join(link);
        let _ = std::fs::remove_dir(&link);
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        let err = store(state.path(), CLAUDE, &user("U1"), &secret("tok-xyz")).unwrap_err();
        assert!(err.contains("not a directory that this user owns"), "{err}");
        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
        assert_eq!(mode(target.path()), 0o755);
    }
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
