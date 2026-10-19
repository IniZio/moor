//! Tests for versioned Artifact storage.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};
use sha2::{Digest, Sha256};

use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Open a store in `dir` with a fake clock.
fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// Mode bits of `path`.
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn first_add_is_version_one_with_content_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let root = dir.path().join("artifacts");
    let bytes = b"hello";
    let row = store
        .add_artifact(t, "shot.png", bytes, &root)
        .unwrap()
        .unwrap();
    let box_dir = root.join(t.get().to_string());
    let file = box_dir.join("v1").join("shot.png");
    assert_eq!(row.version, 1);
    assert_eq!(row.size, 5);
    assert_eq!(row.created_at, Ts::from_millis(1_000));
    assert_eq!(row.kind, "screenshot");
    assert_eq!(row.content_type, "image/png");
    assert_eq!(Path::new(&row.path), file);
    assert_eq!(fs::read(&file).unwrap(), bytes);
}

#[test]
fn stale_file_without_row_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let version_dir = dir.path().join(t.get().to_string()).join("v1");
    fs::create_dir_all(&version_dir).unwrap();
    let file = version_dir.join("a.log");
    fs::write(&file, b"stale").unwrap();
    let row = store
        .add_artifact(t, "a.log", b"fresh", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(row.version, 1);
    assert_eq!(fs::read(&file).unwrap(), b"fresh");
}

#[test]
fn row_holds_digest_and_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let row = store
        .add_artifact(t, "a.log", b"hello", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(row.sha256, format!("{:x}", Sha256::digest(b"hello")));
    assert_eq!(store.artifacts(t).unwrap(), vec![row.clone()]);
    assert_eq!(store.latest_artifacts(t).unwrap(), vec![row]);
}

#[test]
fn files_and_dirs_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let root = dir.path().join("artifacts");
    store.add_artifact(t, "a.log", b"x", &root).unwrap();
    let box_dir = root.join(t.get().to_string());
    assert_eq!(mode(&box_dir.join("v1").join("a.log")), 0o600);
    assert_eq!(mode(&box_dir), 0o700);
    assert_eq!(mode(&box_dir.join("v1")), 0o700);
}

#[test]
fn same_bytes_add_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store
        .add_artifact(t, "a.log", b"x", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(
        store.add_artifact(t, "a.log", b"x", dir.path()).unwrap(),
        None
    );
    assert!(!dir.path().join(t.get().to_string()).join("v2").exists());
    assert_eq!(store.artifacts(t).unwrap().len(), 1);
}

#[test]
fn changed_bytes_make_the_next_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store
        .add_artifact(t, "a.log", b"x", dir.path())
        .unwrap()
        .unwrap();
    let v2 = store
        .add_artifact(t, "a.log", b"y", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(fs::read(&v2.path).unwrap(), b"y");
    assert_eq!(store.artifacts(t).unwrap().len(), 2);
    assert_eq!(store.latest_artifacts(t).unwrap(), vec![v2]);
}

#[test]
fn names_version_independently() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    store.add_artifact(t, "a.log", b"1", dir.path()).unwrap();
    store.add_artifact(t, "a.log", b"2", dir.path()).unwrap();
    let b = store
        .add_artifact(t, "b.log", b"1", dir.path())
        .unwrap()
        .unwrap();
    assert_eq!(b.version, 1);
    let latest: Vec<_> = store
        .latest_artifacts(t)
        .unwrap()
        .into_iter()
        .map(|r| (r.name, r.version))
        .collect();
    assert_eq!(
        latest,
        vec![("a.log".to_owned(), 2), ("b.log".to_owned(), 1)]
    );
}

#[test]
fn invalid_name_is_rejected_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let root = dir.path().join("artifacts");
    for name in ["", "../x", "a/b", ".."] {
        assert!(store.add_artifact(t, name, b"x", &root).is_err(), "{name}");
    }
    assert!(!root.exists());
    assert!(store.artifacts(t).unwrap().is_empty());
}

#[test]
fn symlinked_box_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let root = dir.path().join("artifacts");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, root.join(t.get().to_string())).unwrap();
    let err = store
        .add_artifact(t, "a.log", b"secret", &root)
        .unwrap_err();
    assert!(err.to_string().contains(&t.get().to_string()), "{err}");
    assert!(!err.to_string().contains("secret"));
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(store.artifacts(t).unwrap().is_empty());
}

#[test]
fn add_records_event_and_writes_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let t = add(&store, "a");
    let row = store
        .add_artifact(t, "a.log", b"x", dir.path())
        .unwrap()
        .unwrap();
    let events = store.events(t, 50).unwrap();
    let ev = events
        .iter()
        .find(|e| e.kind == "artifact_published")
        .unwrap();
    assert_eq!(
        ev.payload,
        serde_json::json!({
            "name": "a.log",
            "version": 1,
            "kind": row.kind,
            "content_type": row.content_type,
            "size": 1,
            "sha256": row.sha256,
        })
    );
    let manifest = dir
        .path()
        .join(t.get().to_string())
        .join("artifact-manifest.json");
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
    assert_eq!(doc.get("files").unwrap().as_array().unwrap().len(), 1);
}
