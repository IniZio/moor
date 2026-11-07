#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::seam::clock::Ts;

use super::settle;
use crate::proxy::fronts::ca_dir;
use crate::proxy::nono::ca;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::vault::Vault;

struct Rig {
    dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    Rig { dir, store, vault }
}

impl Rig {
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn settle(&self) -> Result<(), String> {
        settle(&self.store, &self.vault, &self.state(), true)
    }

    fn ssh(&self) -> PathBuf {
        self.state().join("pool").join("id_ed25519")
    }

    fn ssh_files(&self) -> (String, String) {
        let key = self.ssh();
        (
            fs::read_to_string(&key).unwrap(),
            fs::read_to_string(key.with_extension("pub")).unwrap(),
        )
    }

    fn ca_files(&self) -> (String, String) {
        let (key, cert) = ca::read_in(&ca_dir(&self.state())).unwrap();
        (String::from_utf8(key.to_vec()).unwrap(), cert)
    }

    fn wipe(&self) {
        fs::remove_dir_all(self.state()).unwrap();
    }
}

fn keygen(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "old"])
        .arg("-f")
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(made.success());
}

#[test]
fn the_ssh_key_is_the_same_after_an_empty_state_dir_and_a_restart() {
    let rig = rig();
    rig.settle().unwrap();
    let first = rig.ssh_files();
    rig.wipe();
    rig.settle().unwrap();
    assert_eq!(rig.ssh_files(), first);
}

#[test]
fn the_front_ca_is_the_same_after_an_empty_state_dir_and_a_restart() {
    let rig = rig();
    rig.settle().unwrap();
    let first = rig.ca_files();
    rig.wipe();
    rig.settle().unwrap();
    assert_eq!(rig.ca_files(), first);
}

#[test]
fn files_from_an_older_controller_are_imported_not_replaced() {
    let rig = rig();
    keygen(&rig.ssh());
    let old = rig.ssh_files();
    ca::load_or_create_in(&ca_dir(&rig.state()), "moor front proxy").unwrap();
    let old_ca = rig.ca_files();

    rig.settle().unwrap();
    assert_eq!(
        (rig.ssh_files(), rig.ca_files()),
        (old.clone(), old_ca.clone())
    );

    rig.wipe();
    rig.settle().unwrap();
    assert_eq!((rig.ssh_files(), rig.ca_files()), (old, old_ca));
}

#[test]
fn a_stored_value_wins_over_a_file_that_differs() {
    let rig = rig();
    rig.settle().unwrap();
    let (kept, kept_ca) = (rig.ssh_files(), rig.ca_files());
    rig.wipe();
    keygen(&rig.ssh());
    ca::load_or_create_in(&ca_dir(&rig.state()), "moor front proxy").unwrap();
    assert_ne!(rig.ssh_files(), kept);
    assert_ne!(rig.ca_files(), kept_ca);

    rig.settle().unwrap();
    assert_eq!(rig.ssh_files(), kept);
    assert_eq!(rig.ca_files(), kept_ca);
}

#[test]
fn a_vault_that_cannot_open_the_values_refuses_and_makes_nothing() {
    let rig = rig();
    rig.settle().unwrap();
    let kept = rig.ssh_files();
    rig.wipe();
    let other = Vault::load_or_create(&rig.dir.path().join("other.key")).unwrap();

    let err = settle(&rig.store, &other, &rig.state(), true).unwrap_err();
    assert!(err.contains("vault"), "{err}");
    assert!(!rig.ssh().exists(), "no key was made");
    assert!(!ca_dir(&rig.state()).exists(), "no CA was made");

    rig.settle().unwrap();
    assert_eq!(rig.ssh_files(), kept);
}
