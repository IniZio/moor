//! Tests for the vault key file and sealing values to it.

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};

use moor_core::user::{SecretName, UserId};
use moor_core::Secret;

use super::{load_or_create_as, Vault, VaultError};

/// This process's effective uid.
fn me() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[test]
fn a_missing_key_is_created_owner_only_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.key");
    let first = Vault::load_or_create(&path).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let sealed = first
        .seal(USER, &claude(), &Secret::new(b"tok".to_vec()))
        .unwrap();
    let again = Vault::load_or_create(&path).unwrap();
    assert_eq!(
        again.open(USER, &claude(), &sealed).unwrap().expose(),
        b"tok"
    );
}

const USER: UserId = UserId::new(1);

fn claude() -> SecretName {
    SecretName::parse("claude").unwrap()
}

#[test]
fn a_sealed_value_holds_no_plaintext_and_needs_its_own_key() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::load_or_create(&dir.path().join("a.key")).unwrap();
    let other = Vault::load_or_create(&dir.path().join("b.key")).unwrap();
    let sealed = vault
        .seal(USER, &claude(), &Secret::new(b"plain-value-123".to_vec()))
        .unwrap();
    assert!(!sealed.windows(15).any(|w| w == b"plain-value-123"));
    assert!(matches!(
        other.open(USER, &claude(), &sealed),
        Err(VaultError::Open(_))
    ));
}

#[test]
fn a_value_opens_only_for_the_user_and_name_it_was_sealed_for() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let sealed = vault
        .seal(USER, &claude(), &Secret::new(b"tok".to_vec()))
        .unwrap();
    let github = SecretName::parse("github").unwrap();
    let claude2 = SecretName::parse("claude2").unwrap();
    for (user, name) in [
        (UserId::new(2), claude()),
        (UserId::new(11), claude()),
        (USER, github),
        (USER, claude2),
    ] {
        let err = vault.open(user, &name, &sealed).err().expect("refused");
        assert!(err.to_string().contains("another user or name"), "{err}");
    }
}

/// The refusal for an existing `vault.key` that `setup` prepared, with `owner` as the owner.
fn refusal(setup: impl FnOnce(&std::path::Path), owner: u32) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.key");
    setup(&path);
    let before = fs::symlink_metadata(&path).unwrap();
    let err = load_or_create_as(&path, owner)
        .err()
        .expect("vault key refused");
    let after = fs::symlink_metadata(&path).unwrap();
    assert_eq!(
        (before.mode(), before.len()),
        (after.mode(), after.len()),
        "left as it is"
    );
    err.to_string()
}

/// A real key at `path` with mode `mode`.
fn key_with_mode(path: &std::path::Path, mode: u32) {
    Vault::load_or_create(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_key_open_to_group_or_others_is_refused() {
    for mode in [0o644, 0o640, 0o604] {
        let err = refusal(|p| key_with_mode(p, mode), me());
        assert!(err.contains("it must be 0600"), "{err}");
    }
}

#[test]
fn a_symlinked_key_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.key");
    Vault::load_or_create(&real).unwrap();
    let err = refusal(|p| symlink(&real, p).unwrap(), me());
    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn a_key_owned_by_another_user_is_refused() {
    let err = refusal(|p| key_with_mode(p, 0o600), me() + 1);
    assert!(err.contains("is owned by uid"), "{err}");
}

#[test]
fn a_directory_or_garbage_in_place_of_the_key_is_refused() {
    let err = refusal(|p| fs::create_dir(p).unwrap(), me());
    assert!(err.contains("not a regular file"), "{err}");
    let err = refusal(
        |p| {
            fs::write(p, "not a key\n").unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        },
        me(),
    );
    assert!(err.contains("not an age X25519 identity"), "{err}");
    assert!(!err.contains("not a key"), "{err}");
}

#[test]
fn the_proxy_load_refuses_a_missing_or_open_key_and_creates_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.key");
    assert!(Vault::load(&path).is_err());
    assert!(!path.exists(), "a missing key was created");
    key_with_mode(&path, 0o644);
    let err = Vault::load(&path).err().expect("an open key is refused");
    assert!(err.to_string().contains("0600"), "{err}");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Vault::load(&path).is_ok());
}
