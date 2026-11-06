//! Tests for users, identities and sealed user secrets.

use moor_core::user::{Email, Identity, SecretName, UserId};
use moor_core::Secret;
use moor_schema::tables::{UserMachinesV2, UserSecrets, Users};
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{self, DbExt as _};
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
    let (user, created) = store.enroll(&id("local:1000"), None).unwrap();
    assert!(created);
    assert_eq!(
        store.enroll(&id("local:1000"), None).unwrap(),
        (user, false)
    );
    let (other, created) = store.enroll(&id("slack:T1:U1"), None).unwrap();
    assert!(created);
    assert_ne!(other, user);
    assert_eq!(store.user_of(&id("local:1000")).unwrap(), Some(user));
    assert_eq!(store.user_of(&id("web:x")).unwrap(), None);
}

fn email(s: &str) -> Email {
    Email::parse(s).unwrap()
}

#[test]
fn identities_with_one_email_are_one_user() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = email("alice@example.com");
    let (first, created) = store.enroll(&id("slack:T1:U1"), Some(&alice)).unwrap();
    assert!(created);
    let (second, created) = store.enroll(&id("authgear:sub-1"), Some(&alice)).unwrap();
    assert_eq!((second, created), (first, false));
    assert_eq!(
        store.identities(first).unwrap(),
        vec![id("authgear:sub-1"), id("slack:T1:U1")]
    );
    assert_eq!(store.user_of(&id("authgear:sub-1")).unwrap(), Some(first));
}

#[test]
fn an_identity_without_an_email_gets_its_own_user() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = email("alice@example.com");
    let (first, _) = store.enroll(&id("slack:T1:U1"), Some(&alice)).unwrap();
    let (third, created) = store.enroll(&id("web:bob"), None).unwrap();
    assert!(created);
    assert_ne!(third, first);
    let (fourth, _) = store.enroll(&id("web:carol"), None).unwrap();
    assert_ne!(fourth, third);
}

#[test]
fn an_email_is_matched_whatever_its_letter_case() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (first, _) = store
        .enroll(&id("slack:T1:U1"), Some(&email("Alice@Example.com")))
        .unwrap();
    let (second, _) = store
        .enroll(&id("web:alice"), Some(&email("alice@example.COM")))
        .unwrap();
    assert_eq!(second, first);
}

#[test]
fn a_known_identity_gains_an_email_it_lacked() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (user, _) = store.enroll(&id("slack:T1:U1"), None).unwrap();
    let alice = email("alice@example.com");
    assert_eq!(
        store.enroll(&id("slack:T1:U1"), Some(&alice)).unwrap(),
        (user, false)
    );
    let (joined, created) = store.enroll(&id("authgear:sub-1"), Some(&alice)).unwrap();
    assert_eq!((joined, created), (user, false));
}

#[test]
fn a_known_identity_never_takes_an_email_another_user_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let alice = email("alice@example.com");
    let (holder, _) = store.enroll(&id("web:alice"), Some(&alice)).unwrap();
    let (other, _) = store.enroll(&id("slack:T1:U2"), None).unwrap();
    assert_eq!(
        store.enroll(&id("slack:T1:U2"), Some(&alice)).unwrap(),
        (other, false)
    );
    assert_ne!(other, holder);
    assert_eq!(
        store.enroll(&id("authgear:sub-1"), Some(&alice)).unwrap(),
        (holder, false)
    );
}

#[test]
fn the_database_refuses_two_users_with_one_email() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    store
        .enroll(&id("web:alice"), Some(&email("alice@example.com")))
        .unwrap();
    let raw = store.writer();
    let duplicate = raw.run(
        Query::insert()
            .into_table(Users::Table)
            .columns([Users::CreatedAt, Users::Email])
            .values_panic([1.into(), "alice@example.com".into()]),
    );
    assert!(duplicate.is_err());
    for _ in 0..2 {
        raw.run(
            Query::insert()
                .into_table(Users::Table)
                .columns([Users::CreatedAt])
                .values_panic([1.into()]),
        )
        .unwrap();
    }
}

#[test]
fn user_secrets_are_stored_sealed_and_opened_on_every_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let (user, _) = store.enroll(&id("local:1000"), None).unwrap();
    let (other, _) = store.enroll(&id("web:bob"), None).unwrap();
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
    let (alice, _) = store.enroll(&id("local:1000"), None).unwrap();
    let (bob, _) = store.enroll(&id("web:bob"), None).unwrap();
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
    let (alice, _) = store.enroll(&id("local:1000"), None).unwrap();
    let (bob, _) = store.enroll(&id("web:bob"), None).unwrap();
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
    let (alice, _) = store.enroll(&id("local:1000"), None).unwrap();
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
    let (user, _) = store.enroll(&id("local:1000"), None).unwrap();
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
    let (alice, _) = store.enroll(&id("local:1000"), None).unwrap();
    let (bob, _) = store.enroll(&id("web:bob"), None).unwrap();
    for (user, key) in [(alice, "claude"), (bob, "claude"), (alice, "github")] {
        let value = Secret::new(format!("{}-{key}", user.get()).into_bytes());
        let sealed = vault.seal(user, &name(key), &value).unwrap();
        store.set_user_secret(user, &name(key), &sealed).unwrap();
    }
    let alices = Query::select()
        .column(UserSecrets::Value)
        .from(UserSecrets::Table)
        .and_where(Expr::col(UserSecrets::User).eq(alice.get()))
        .and_where(Expr::col(UserSecrets::Name).eq("claude"))
        .take();
    let named = |user: UserId, key: &str| {
        Expr::col(UserSecrets::User)
            .eq(user.get())
            .and(Expr::col(UserSecrets::Name).eq(key))
    };
    let copied = store
        .writer()
        .run(
            Query::update()
                .table(UserSecrets::Table)
                .value(UserSecrets::Value, db::sub(alices))
                .and_where(named(bob, "claude").or(named(alice, "github"))),
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

mod hold;
mod links;
mod machine;
mod nodes;
mod overdue;
mod workflow;

#[test]
fn a_link_keeps_its_agent_version_and_names_its_machine() {
    use moor_core::config::EnrolledMachine;
    use moor_core::request::MachineLink;
    use moor_core::seam::clock::Ts;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, _) = store.enroll(&id("authgear:a"), None).unwrap();
    let (bob, _) = store.enroll(&id("authgear:b"), None).unwrap();
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
    let m = EnrolledMachine::parse("host", key, &[], 1).unwrap();
    let machine_id = store.enroll_machine(alice, &m).unwrap();
    let link = MachineLink {
        machine_id,
        online: true,
        since: Ts::from_millis(5),
        version: Some("0.9.1".to_owned()),
        protocol: Some(1),
        max_boxes: Some(4),
        cache_gb: Some(50),
        features: vec!["cache-volume".to_owned(), "ssh".to_owned()],
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    let held = store
        .set_machine_links(
            &crate::testing::runner(),
            false,
            std::slice::from_ref(&link),
            store.now(),
        )
        .unwrap();
    assert_eq!(held, vec![link.clone()]);
    assert_eq!(
        store.user_machine_links(alice).unwrap(),
        vec![("host".to_owned(), link)]
    );
    assert_eq!(store.user_machine_links(bob).unwrap(), vec![]);
}

#[test]
fn machine_links_are_replaced_whole_and_skip_unknown_machines() {
    use moor_core::config::EnrolledMachine;
    use moor_core::request::MachineLink;
    use moor_core::seam::clock::Ts;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, _) = store.enroll(&id("authgear:a"), None).unwrap();
    let keys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ];
    let [key_one, key_two] = keys;
    let [one, two] = [("one", key_one), ("two", key_two)].map(|(name, key)| {
        let m = EnrolledMachine::parse(name, key, &[], 1).unwrap();
        store.enroll_machine(alice, &m).unwrap()
    });
    let link = |machine_id, online, since| MachineLink {
        machine_id,
        online,
        since: Ts::from_millis(since),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    let set = |links: &[MachineLink]| {
        let runner = crate::testing::runner();
        store.set_machine_links(&runner, false, links, store.now())
    };
    let held = set(&[link(one, true, 5), link(two, false, 6), link(999, true, 7)]).unwrap();
    assert_eq!(held, vec![link(one, true, 5), link(two, false, 6)]);
    let held = set(&[link(two, true, 9)]).unwrap();
    assert_eq!(held, vec![link(two, true, 9)]);
    let listed: Vec<(i64, String, String)> = store
        .machine_keys()
        .unwrap()
        .into_iter()
        .map(|k| {
            assert_eq!(k.user, alice);
            (k.machine_id, k.name, k.pubkey)
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            (one, "one".to_owned(), key_one.to_owned()),
            (two, "two".to_owned(), key_two.to_owned()),
        ]
    );
}

#[test]
fn enrolling_a_machine_again_replaces_its_key_and_keeps_its_id() {
    use moor_core::config::EnrolledMachine;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (alice, _) = store.enroll(&id("authgear:a"), None).unwrap();
    let (bob, _) = store.enroll(&id("authgear:b"), None).unwrap();
    let machine = |key: &str, capacity| {
        EnrolledMachine::parse("minion", key, &["macos".to_owned()], capacity).unwrap()
    };
    let first = store
        .enroll_machine(
            alice,
            &machine(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
                1,
            ),
        )
        .unwrap();
    let again = store
        .enroll_machine(
            alice,
            &machine(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                3,
            ),
        )
        .unwrap();
    let bobs = store
        .enroll_machine(
            bob,
            &machine(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
                1,
            ),
        )
        .unwrap();
    assert_eq!(again, first, "the same name keeps its id");
    assert_ne!(bobs, first, "names are per user");
    let rows: Vec<(String, u16)> = store
        .reader()
        .unwrap()
        .rows(
            Query::select()
                .columns([UserMachinesV2::Pubkey, UserMachinesV2::Capacity])
                .from(UserMachinesV2::Table)
                .and_where(Expr::col(UserMachinesV2::MachineId).eq(first)),
            |r| Ok::<_, db::Error>((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_owned(),
            3
        )]
    );
}
