//! Tests for `artifacts` and `artifact-read`.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::{ArtifactReadReq, ArtifactsReq, ARTIFACT_CHUNK};
use moor_core::{BoxId, BoxName, ErrorKind, Request};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{list, read};
use crate::cx::Cx;
use crate::ops::dispatch;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Path::new("/home/me/proj"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
        Arc::new(ScriptedProc::new()),
    )
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

fn task(cx: &Cx, n: &str) -> BoxId {
    let nt = NewBox {
        name: name(n),
        repo: "r".into(),
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .task
}

fn publish(cx: &Cx, t: BoxId, file: &str, bytes: &[u8]) {
    let root = cx.state_dir().unwrap().join("artifacts");
    cx.store()
        .unwrap()
        .add_artifact(t, file, bytes, &root)
        .unwrap();
}

fn list_req(n: &str, all: bool) -> ArtifactsReq {
    ArtifactsReq {
        box_name: name(n),
        all,
    }
}

fn read_req(n: &str, file: &str, version: Option<u32>, offset: u64, len: u64) -> ArtifactReadReq {
    ArtifactReadReq {
        box_name: name(n),
        name: file.into(),
        version,
        offset,
        len,
    }
}

#[test]
fn list_shows_the_latest_version_or_every_version() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    publish(&cx, t, "log.txt", b"two");
    publish(&cx, t, "shot.png", b"png");
    let latest = list(&cx, &list_req("a", false)).unwrap();
    let got: Vec<_> = latest
        .iter()
        .map(|v| (v.name.as_str(), v.version))
        .collect();
    assert_eq!(got, [("log.txt", 2), ("shot.png", 1)]);
    let all = list(&cx, &list_req("a", true)).unwrap();
    let got: Vec<_> = all.iter().map(|v| (v.name.as_str(), v.version)).collect();
    assert_eq!(got, [("log.txt", 1), ("log.txt", 2), ("shot.png", 1)]);
    let first = latest.first().unwrap();
    assert_eq!(first.size, 3);
    assert_eq!(first.sha256, format!("{:x}", Sha256::digest(b"two")));
    assert!(first.created_at.ends_with('Z'));
}

#[test]
fn dispatch_returns_the_wire_shape() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"hello");
    let got = dispatch(&cx, Request::Artifacts(list_req("a", false))).unwrap();
    assert_eq!(got.pointer("/0/name"), Some(&json!("log.txt")));
    let got = dispatch(
        &cx,
        Request::ArtifactRead(read_req("a", "log.txt", None, 0, 99)),
    )
    .unwrap();
    assert_eq!(got.get("data_b64"), Some(&json!(STANDARD.encode(b"hello"))));
    assert_eq!(got.get("eof"), Some(&json!(true)));
}

#[test]
fn chunked_reads_reassemble_to_the_stored_digest() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    let bytes: Vec<u8> = (0..100u8).collect();
    publish(&cx, t, "data.bin", &bytes);
    let mut got = Vec::new();
    let hash = loop {
        let c = read(&cx, &read_req("a", "data.bin", None, got.len() as u64, 7)).unwrap();
        got.extend(STANDARD.decode(&c.data_b64).unwrap());
        if c.eof {
            break c.sha256;
        }
    };
    assert_eq!(got, bytes);
    assert_eq!(hash, format!("{:x}", Sha256::digest(&bytes)));
}

#[test]
fn a_named_version_is_read_not_the_latest() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    publish(&cx, t, "log.txt", b"two");
    let c = read(&cx, &read_req("a", "log.txt", Some(1), 0, 10)).unwrap();
    assert_eq!(STANDARD.decode(c.data_b64).unwrap(), b"one");
}

#[test]
fn a_length_above_the_chunk_cap_is_clamped() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "big.bin", &vec![7u8; 600 * 1024]);
    let c = read(&cx, &read_req("a", "big.bin", None, 0, u64::MAX)).unwrap();
    assert_eq!(
        STANDARD.decode(c.data_b64).unwrap().len() as u64,
        ARTIFACT_CHUNK
    );
    assert!(!c.eof);
    assert_eq!(c.size, 600 * 1024);
}

#[test]
fn an_offset_at_the_end_is_empty_eof_and_past_it_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"abc");
    let c = read(&cx, &read_req("a", "log.txt", None, 3, 5)).unwrap();
    assert!(c.eof && c.data_b64.is_empty());
    let e = read(&cx, &read_req("a", "log.txt", None, 4, 5)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}

#[test]
fn unknown_task_name_or_version_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let e = list(&cx, &list_req("nope", false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = read(&cx, &read_req("nope", "log.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = read(&cx, &read_req("a", "other.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = read(&cx, &read_req("a", "log.txt", Some(9), 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn a_missing_file_on_disk_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let file = dir
        .path()
        .join("artifacts")
        .join(t.get().to_string())
        .join("v1/log.txt");
    fs::remove_file(file).unwrap();
    let e = read(&cx, &read_req("a", "log.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn a_bad_name_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    task(&cx, "a");
    for bad in ["../x", "a/b", ""] {
        let e = read(&cx, &read_req("a", bad, None, 0, 1)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Usage, "{bad}");
    }
}

#[test]
fn a_symlinked_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let secret = dir.path().join("secret");
    fs::write(&secret, b"s3cret").unwrap();
    let file = dir
        .path()
        .join("artifacts")
        .join(t.get().to_string())
        .join("v1/log.txt");
    fs::remove_file(&file).unwrap();
    symlink(&secret, &file).unwrap();
    let e = read(&cx, &read_req("a", "log.txt", None, 0, 99)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(!e.error.to_string().contains("s3cret"));
}

#[test]
fn a_symlinked_version_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = task(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let box_dir = dir.path().join("artifacts").join(t.get().to_string());
    let moved = dir.path().join("elsewhere");
    fs::rename(box_dir.join("v1"), &moved).unwrap();
    symlink(&moved, box_dir.join("v1")).unwrap();
    let e = read(&cx, &read_req("a", "log.txt", None, 0, 99)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
}
