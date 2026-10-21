//! Tests for users, identities, link codes and sealed user secrets.

use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::Secret;

use crate::store::tests::open;
use crate::store::{Store, StoreError};
use crate::vault::{Vault, VaultError};

fn id(s: &str) -> Identity {
    Identity::parse(s).unwrap()
}

fn name(s: &str) -> SecretName {
    SecretName::parse(s).unwrap()
}

#[test]
fn enroll_creates_a_user_once_per_identity() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (user, created) = store.enroll(&id("local:1000")).unwrap();
    assert!(created);
    assert_eq!(store.enroll(&id("local:1000")).unwrap(), (user, false));
    let (other, created) = store.enroll(&id("slack:T1:U1")).unwrap();
    assert!(created);
    assert_ne!(other, user);
    assert_eq!(store.user_of(&id("local:1000")).unwrap(), Some(user));
    assert_eq!(store.user_of(&id("web:x")).unwrap(), None);
}

#[test]
fn a_link_code_links_a_second_identity_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let (user, _) = store.enroll(&id("local:1000")).unwrap();
    let expires = clock.now().saturating_add(Duration::from_secs(600));
    store.add_link_code(user, "code-1", expires).unwrap();
    assert_eq!(
        store
            .redeem_link_code("code-1", &id("slack:T1:U1"))
            .unwrap(),
        user
    );
    assert_eq!(
        store.identities(user).unwrap(),
        vec![id("local:1000"), id("slack:T1:U1")]
    );
    let again = store
        .redeem_link_code("code-1", &id("web:alice"))
        .unwrap_err();
    assert!(
        matches!(&again, StoreError::Invalid(why) if why == "unknown or used link code"),
        "{again:?}"
    );
    assert_eq!(store.user_of(&id("web:alice")).unwrap(), None);
}

#[test]
fn an_expired_link_code_is_refused_and_used_up() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let (user, _) = store.enroll(&id("local:1000")).unwrap();
    let expires = clock.now().saturating_add(Duration::from_secs(600));
    store.add_link_code(user, "code-1", expires).unwrap();
    clock.sleep(Duration::from_secs(600));
    let err = store
        .redeem_link_code("code-1", &id("web:alice"))
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(why) if why.contains("expired")),
        "{err:?}"
    );
    assert_eq!(store.user_of(&id("web:alice")).unwrap(), None);
    let again = store
        .redeem_link_code("code-1", &id("web:alice"))
        .unwrap_err();
    assert!(
        matches!(&again, StoreError::Invalid(why) if why.contains("unknown")),
        "{again:?}"
    );
}

#[test]
fn a_linked_identity_cannot_redeem_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (user, _) = store.enroll(&id("local:1000")).unwrap();
    let (other, _) = store.enroll(&id("web:alice")).unwrap();
    store
        .add_link_code(user, "code-1", Ts::from_millis(i64::MAX))
        .unwrap();
    let err = store
        .redeem_link_code("code-1", &id("web:alice"))
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(why) if why.contains("already linked")),
        "{err:?}"
    );
    assert_eq!(store.user_of(&id("web:alice")).unwrap(), Some(other));
}

#[test]
fn user_secrets_are_stored_sealed_and_opened_on_every_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (user, _) = store.enroll(&id("local:1000")).unwrap();
    let (other, _) = store.enroll(&id("web:bob")).unwrap();
    let value = "sk-plaintext-canary-7f3e";
    let sealed = vault
        .seal(user, &name("claude"), &Secret::new(value.into()))
        .unwrap();
    store
        .set_user_secret(user, &name("claude"), &sealed)
        .unwrap();
    let got = store.user_secret(&vault, user, &name("claude")).unwrap();
    assert_eq!(got.unwrap().expose(), value.as_bytes());
    assert!(store
        .user_secret(&vault, other, &name("claude"))
        .unwrap()
        .is_none());
    assert_eq!(store.user_secret_names(user).unwrap(), vec![name("claude")]);
    for file in ["moor.db", "moor.db-wal"] {
        let bytes = std::fs::read(dir.path().join("state").join(file)).unwrap_or_default();
        assert!(
            !bytes.windows(value.len()).any(|w| w == value.as_bytes()),
            "{file} holds the plaintext"
        );
    }
    assert!(store.delete_user_secret(user, &name("claude")).unwrap());
    assert!(!store.delete_user_secret(user, &name("claude")).unwrap());
    assert!(store
        .user_secret(&vault, user, &name("claude"))
        .unwrap()
        .is_none());
}

fn put(store: &Store, vault: &Vault, user: UserId, key: &str, value: &str) {
    let sealed = vault
        .seal(user, &name(key), &Secret::new(value.into()))
        .unwrap();
    store.set_user_secret(user, &name(key), &sealed).unwrap();
}

#[test]
fn listing_secret_names_shows_only_the_users_own() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (alice, _) = store.enroll(&id("local:1000")).unwrap();
    let (bob, _) = store.enroll(&id("web:bob")).unwrap();
    put(&store, &vault, alice, "github", "a-github");
    put(&store, &vault, bob, "claude", "b-claude");
    put(&store, &vault, bob, "openai", "b-openai");
    assert_eq!(
        store.user_secret_names(alice).unwrap(),
        vec![name("github")]
    );
    assert_eq!(
        store.user_secret_names(bob).unwrap(),
        vec![name("claude"), name("openai")]
    );
}

#[test]
fn deleting_a_secret_leaves_another_users_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (alice, _) = store.enroll(&id("local:1000")).unwrap();
    let (bob, _) = store.enroll(&id("web:bob")).unwrap();
    put(&store, &vault, alice, "github", "a-github");
    put(&store, &vault, bob, "github", "b-github");
    let deleted = store.delete_user_secret(alice, &name("github")).unwrap();
    let kept = store.user_secret(&vault, bob, &name("github")).unwrap();
    assert_eq!(
        kept.map(|s| s.expose().to_vec()),
        Some(b"b-github".to_vec())
    );
    assert!(deleted);
    assert!(store
        .user_secret(&vault, alice, &name("github"))
        .unwrap()
        .is_none());
}

#[test]
fn each_secret_name_opens_to_its_own_value() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (alice, _) = store.enroll(&id("local:1000")).unwrap();
    put(&store, &vault, alice, "claude", "a-claude");
    put(&store, &vault, alice, "github", "a-github");
    for (key, value) in [("claude", "a-claude"), ("github", "a-github")] {
        let got = store.user_secret(&vault, alice, &name(key)).unwrap();
        assert_eq!(got.unwrap().expose(), value.as_bytes(), "{key}");
    }
}

#[test]
fn setting_a_secret_again_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (user, _) = store.enroll(&id("local:1000")).unwrap();
    for value in ["one", "two"] {
        let sealed = vault
            .seal(user, &name("github"), &Secret::new(value.into()))
            .unwrap();
        store
            .set_user_secret(user, &name("github"), &sealed)
            .unwrap();
    }
    let got = store.user_secret(&vault, user, &name("github")).unwrap();
    assert_eq!(got.unwrap().expose(), b"two");
}

#[test]
fn a_sealed_value_copied_into_another_row_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (alice, _) = store.enroll(&id("local:1000")).unwrap();
    let (bob, _) = store.enroll(&id("web:bob")).unwrap();
    for (user, key) in [(alice, "claude"), (bob, "claude"), (alice, "github")] {
        let value = Secret::new(format!("{}-{key}", user.get()).into_bytes());
        let sealed = vault.seal(user, &name(key), &value).unwrap();
        store.set_user_secret(user, &name(key), &sealed).unwrap();
    }
    let db = rusqlite::Connection::open(dir.path().join("state").join("moor.db")).unwrap();
    let copied = db
        .execute(
            "UPDATE user_secrets SET value = \
             (SELECT value FROM user_secrets WHERE user = ?1 AND name = 'claude') \
             WHERE (user = ?2 AND name = 'claude') OR (user = ?1 AND name = 'github')",
            [alice.get(), bob.get()],
        )
        .unwrap();
    assert_eq!(copied, 2);
    for (user, key) in [(bob, "claude"), (alice, "github")] {
        let err = store
            .user_secret(&vault, user, &name(key))
            .err()
            .expect("refused");
        assert!(
            matches!(&err, StoreError::Vault(VaultError::Open(why)) if why.contains("another user or name")),
            "{err:?}"
        );
    }
    let own = store.user_secret(&vault, alice, &name("claude")).unwrap();
    assert_eq!(
        own.unwrap().expose(),
        format!("{}-claude", alice.get()).as_bytes()
    );
}
