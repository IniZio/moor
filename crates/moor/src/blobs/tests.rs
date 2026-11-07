//! The blob store contract.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::fs;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::ArtifactReadReq;
use moor_core::{BoxId, BoxName};
use sha2::{Digest, Sha256};

use super::fs::FsBlobs;
use super::{BlobError, BlobKey, Blobs};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// One byte over the size that goes up in a single request.
const OVER_ONE_PART: usize = 16 * 1024 * 1024 + 1;

/// A context on `blobs` holding one Box.
pub(crate) fn cx_on(dir: &std::path::Path, blobs: Arc<dyn Blobs>) -> (Cx, BoxId) {
    let cx = Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_blobs(blobs);
    let id = cx
        .store()
        .unwrap()
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    (cx, id)
}

fn read_all(blobs: &dyn Blobs, key: &BlobKey, offset: u64, len: u64) -> Vec<u8> {
    let mut got = Vec::new();
    blobs
        .read(key, offset, len, &mut |piece| {
            got.extend_from_slice(piece);
            Ok(())
        })
        .unwrap();
    got
}

fn holds_what_it_was_given(blobs: &dyn Blobs) {
    let key = BlobKey::raw("a/b/c.txt");
    assert!(matches!(blobs.size(&key), Err(BlobError::Missing)));
    blobs.put(&key, b"hello world").unwrap();
    assert_eq!(blobs.size(&key).unwrap(), 11);
    assert_eq!(read_all(blobs, &key, 6, 5), b"world");
    blobs.put(&key, b"replaced").unwrap();
    assert_eq!(read_all(blobs, &key, 0, 8), b"replaced");
}

fn takes_a_file_of_any_size(blobs: &dyn Blobs, dir: &std::path::Path) {
    let bytes: Vec<u8> = (0..OVER_ONE_PART)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    for (name, want) in [("small", &bytes[..10]), ("large", &bytes[..])] {
        let staged = dir.join(name);
        fs::write(&staged, want).unwrap();
        let key = BlobKey::raw(&format!("files/{name}"));
        blobs.put_file(&key, &staged).unwrap();
        assert!(!staged.exists(), "{name} was taken");
        assert_eq!(blobs.size(&key).unwrap(), want.len() as u64);
        let got = read_all(blobs, &key, 0, want.len() as u64);
        assert_eq!(Sha256::digest(&got), Sha256::digest(want), "{name}");
    }
}

fn artifact_round_trip(blobs: Arc<dyn Blobs>) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = cx_on(dir.path(), blobs);
    let store = cx.store().unwrap();
    let staged = dir.path().join("staged");
    fs::write(&staged, b"the artifact").unwrap();
    let sha = format!("{:x}", Sha256::digest(b"the artifact"));
    store
        .add_artifact_staged(id, "a.log", &staged, &sha, cx.blobs().unwrap())
        .unwrap()
        .unwrap();
    let (mut got, req) = (Vec::new(), read_req(0, u64::MAX));
    let view = crate::ops::artifacts::read(&cx, &req, &mut |piece| {
        got.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    assert_eq!(got, b"the artifact");
    assert!(view.eof);
    let mut part = Vec::new();
    crate::ops::artifacts::read(&cx, &read_req(4, 8), &mut |piece| {
        part.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    assert_eq!(part, b"artifact");
    let manifest = BlobKey::artifact_manifest(id);
    let size = cx.blobs().unwrap().size(&manifest).unwrap();
    let doc = read_all(cx.blobs().unwrap(), &manifest, 0, size);
    let doc: serde_json::Value = serde_json::from_slice(&doc).unwrap();
    assert_eq!(doc["storage"]["backend"], cx.blobs().unwrap().name());
}

fn report_round_trip(blobs: Arc<dyn Blobs>) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = cx_on(dir.path(), blobs);
    let (store, blobs) = (cx.store().unwrap(), cx.blobs().unwrap());
    let row = store.add_report(id, "plan", "one", blobs).unwrap().unwrap();
    assert_eq!(row.version, 1);
    assert_eq!(store.add_report(id, "plan", "one", blobs).unwrap(), None);
    let two = store.add_report(id, "plan", "two", blobs).unwrap().unwrap();
    assert_eq!(two.version, 2);
    let got = |version| blobs.get(&BlobKey::report(id, "plan", version)).unwrap();
    assert_eq!((got(1), got(2)), (b"one".to_vec(), b"two".to_vec()));
    assert_eq!(two.path, blobs.locate(&BlobKey::report(id, "plan", 2)));
}

fn read_req(offset: u64, len: u64) -> ArtifactReadReq {
    ArtifactReadReq {
        target: BoxName::parse_allow_bare("a").unwrap().into(),
        name: "a.log".to_owned(),
        version: None,
        offset,
        len,
    }
}

#[test]
fn fs_holds_what_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    holds_what_it_was_given(&FsBlobs::new(dir.path()));
}

#[test]
fn fs_takes_a_file_of_any_size() {
    let (dir, staging) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    takes_a_file_of_any_size(&FsBlobs::new(dir.path()), staging.path());
}

#[test]
fn fs_artifact_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    artifact_round_trip(Arc::new(FsBlobs::new(dir.path())));
}

#[test]
fn fs_report_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    report_round_trip(Arc::new(FsBlobs::new(dir.path())));
}

/// Fails a put if another store write cannot get through while it runs.
struct WritesDuringPut {
    inner: FsBlobs,
    store: crate::store::Store,
    id: BoxId,
}

impl Blobs for WritesDuringPut {
    fn put(&self, key: &BlobKey, bytes: &[u8]) -> Result<(), BlobError> {
        let (sent, got) = std::sync::mpsc::channel();
        let (store, id) = (self.store.clone(), self.id);
        std::thread::spawn(move || {
            drop(sent.send(store.set_artifacts_through(id, 1)));
        });
        got.recv_timeout(std::time::Duration::from_secs(10))
            .expect("a store write waited on the blob put")
            .unwrap();
        self.inner.put(key, bytes)
    }

    fn put_file(&self, key: &BlobKey, from: &std::path::Path) -> Result<(), BlobError> {
        self.inner.put_file(key, from)
    }

    fn size(&self, key: &BlobKey) -> Result<u64, BlobError> {
        self.inner.size(key)
    }

    fn read(
        &self,
        key: &BlobKey,
        offset: u64,
        len: u64,
        sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Result<(), BlobError> {
        self.inner.read(key, offset, len, sink)
    }

    fn locate(&self, key: &BlobKey) -> String {
        self.inner.locate(key)
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

#[test]
fn storing_a_blob_holds_no_store_lock() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = cx_on(dir.path(), Arc::new(FsBlobs::new(dir.path())));
    let store = cx.store().unwrap();
    let blobs = WritesDuringPut {
        inner: FsBlobs::new(dir.path()),
        store: store.clone(),
        id,
    };
    store
        .add_artifact(id, "a.log", b"x", &blobs)
        .unwrap()
        .unwrap();
}
