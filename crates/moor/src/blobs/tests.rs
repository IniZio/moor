//! The blob store contract, on the filesystem and on a real S3-compatible store.
//!
//! The `s3` tests need `MOOR_TEST_S3_ENDPOINT` and run under `make unit-s3`.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::fs;
use std::net::TcpListener;
use std::sync::Arc;

use moor_core::config::{BlobsConfig, S3Blobs};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::ArtifactReadReq;
use moor_core::{BoxId, BoxName};
use sha2::{Digest, Sha256};

use super::fs::FsBlobs;
use super::object::{Credentials, ObjectBlobs};
use super::{BlobError, BlobKey, Blobs};
use crate::cx::Cx;
use crate::env::Env;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// One byte over the size that goes up in a single request.
const OVER_ONE_PART: usize = 16 * 1024 * 1024 + 1;

fn s3_config(endpoint: &str, prefix: &str) -> S3Blobs {
    S3Blobs {
        endpoint: endpoint.to_owned(),
        bucket: std::env::var("MOOR_TEST_S3_BUCKET").unwrap_or_else(|_| "moor-test".to_owned()),
        region: None,
        prefix: Some(prefix.to_owned()),
        credentials_dir: None,
    }
}

fn s3_at(endpoint: &str) -> Arc<dyn Blobs> {
    // A fresh temp name keeps runs against a shared bucket apart.
    let prefix = format!("test-{}", tempfile::tempdir().unwrap().path().display());
    let prefix = prefix.replace('/', "-");
    let credentials = Credentials {
        id: std::env::var("MOOR_TEST_S3_KEY").unwrap_or_else(|_| "moor-test-key".to_owned()),
        secret: std::env::var("MOOR_TEST_S3_SECRET")
            .unwrap_or_else(|_| "moor-test-secret".to_owned()),
    };
    Arc::new(ObjectBlobs::s3(&s3_config(endpoint, &prefix), &credentials).unwrap())
}

/// The real store the environment names, under a prefix of its own.
pub(crate) fn s3() -> Arc<dyn Blobs> {
    s3_at(&std::env::var("MOOR_TEST_S3_ENDPOINT").expect("run under make unit-s3"))
}

/// A store whose endpoint refuses every connection.
pub(crate) fn unreachable() -> Arc<dyn Blobs> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || listener.incoming().for_each(drop));
    s3_at(&format!("http://127.0.0.1:{port}"))
}

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

#[test]
#[ignore = "needs MOOR_TEST_S3_ENDPOINT"]
fn s3_report_round_trip() {
    report_round_trip(s3());
}

#[test]
#[ignore = "needs MOOR_TEST_S3_ENDPOINT"]
fn s3_holds_what_it_was_given() {
    holds_what_it_was_given(s3().as_ref());
}

#[test]
#[ignore = "needs MOOR_TEST_S3_ENDPOINT"]
fn s3_takes_a_file_of_any_size() {
    let staging = tempfile::tempdir().unwrap();
    takes_a_file_of_any_size(s3().as_ref(), staging.path());
}

#[test]
#[ignore = "needs MOOR_TEST_S3_ENDPOINT"]
fn s3_artifact_round_trip() {
    artifact_round_trip(s3());
}

#[test]
fn an_unreachable_store_answers_get_artifact_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = cx_on(dir.path(), unreachable());
    cx.store()
        .unwrap()
        .add_artifact(id, "a.log", b"x", &FsBlobs::new(dir.path()))
        .unwrap()
        .unwrap();
    let err = crate::ops::artifacts::read(&cx, &read_req(0, 1), &mut |_| Ok(())).unwrap_err();
    let text = err.error.to_string();
    assert!(text.contains("cannot read artifact a.log"), "{text}");
    assert!(text.contains("blob store http://127.0.0.1"), "{text}");
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

#[test]
fn an_unreachable_store_leaves_a_report_unrecorded() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = cx_on(dir.path(), unreachable());
    let store = cx.store().unwrap();
    let err = store
        .add_report(id, "plan", "the report body", cx.blobs().unwrap())
        .unwrap_err()
        .to_string();
    assert!(err.contains("blob store http"), "{err}");
    assert!(!err.contains("the report body"), "{err}");
    assert!(store.reports(id).unwrap().is_empty());
}

#[test]
fn an_s3_store_without_credentials_is_refused_at_setup() {
    let config = BlobsConfig::S3(s3_config("http://127.0.0.1:1", "p"));
    let err = super::share(Env::from_pairs([("HOME", "/nowhere")]), &config)
        .err()
        .unwrap();
    assert!(err.contains("MOOR_BLOBS_ACCESS_KEY_ID"), "{err}");
}

#[test]
fn a_controller_sharing_a_bucket_keeps_its_store() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("HOME", "/nowhere"),
        ("MOOR_STATE_DIR", dir.path().to_str().unwrap()),
        ("MOOR_BLOBS_ACCESS_KEY_ID", "id"),
        ("MOOR_BLOBS_SECRET_ACCESS_KEY", "secret"),
    ]);
    let env = crate::Moor::open_store(env).unwrap();
    let config = BlobsConfig::S3(s3_config("http://127.0.0.1:1", "p"));
    let env = super::share(env, &config).unwrap();
    assert!(env.shared::<crate::store::Store>().is_some());
    assert!(env.shared::<Arc<dyn Blobs>>().is_some());
}

#[test]
fn an_s3_store_reads_a_mounted_secret() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("access_key_id"), "id\n").unwrap();
    fs::write(dir.path().join("secret_access_key"), "secret\n").unwrap();
    let mut config = s3_config("http://127.0.0.1:1", "p");
    config.credentials_dir = Some(dir.path().display().to_string());
    let got =
        super::object::credentials(&config, &Env::from_pairs([("HOME", "/nowhere")])).unwrap();
    assert_eq!((got.id.as_str(), got.secret.as_str()), ("id", "secret"));
}
