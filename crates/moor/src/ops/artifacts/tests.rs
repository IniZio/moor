//! Tests for `artifacts` and `artifact-read`.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::{ArtifactReadReq, ArtifactsReq};
use moor_core::view::ArtifactReadView;
use moor_core::{BoxId, BoxName, ErrorKind, Request};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{list, read};
use crate::cx::Cx;
use crate::error::CommandError;
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

fn new_box(cx: &Cx, n: &str) -> BoxId {
    let nt = NewBox {
        name: name(n),
        repo: "r".into(),
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .box_id
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
        target: name(n).into(),
        all,
    }
}

fn read_req(n: &str, file: &str, version: Option<u32>, offset: u64, len: u64) -> ArtifactReadReq {
    ArtifactReadReq {
        target: name(n).into(),
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
    let t = new_box(&cx, "a");
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

/// Run `read`, collecting each piece it sends.
fn pieces(
    cx: &Cx,
    req: &ArtifactReadReq,
) -> Result<(Vec<Vec<u8>>, ArtifactReadView), CommandError> {
    let mut got = Vec::new();
    let view = read(cx, req, &mut |p| {
        got.push(p.to_vec());
        Ok(())
    })?;
    Ok((got, view))
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn dispatch_lists_but_refuses_a_read_it_cannot_stream() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"hello");
    let got = dispatch(&cx, Request::Artifacts(list_req("a", false))).unwrap();
    assert_eq!(got.pointer("/0/name"), Some(&json!("log.txt")));
    let e = dispatch(
        &cx,
        Request::ArtifactRead(read_req("a", "log.txt", None, 0, 99)),
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}

#[test]
fn a_whole_file_streams_in_pieces_no_larger_than_a_bytes_frame() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    let bytes: Vec<u8> = (0..150 * 1024)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    publish(&cx, t, "data.bin", &bytes);
    let (got, view) = pieces(&cx, &read_req("a", "data.bin", None, 0, u64::MAX)).unwrap();
    let lens: Vec<usize> = got.iter().map(Vec::len).collect();
    assert_eq!(lens, [64 * 1024, 64 * 1024, 22 * 1024]);
    assert_eq!(got.concat(), bytes);
    let want = ArtifactReadView {
        offset: 0,
        sent: 150 * 1024,
        size: 150 * 1024,
        sha256: sha(&bytes),
        eof: true,
    };
    assert_eq!(view, want);
}

#[test]
fn a_range_sends_only_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    let bytes: Vec<u8> = (0..100u8).collect();
    publish(&cx, t, "data.bin", &bytes);
    let (got, view) = pieces(&cx, &read_req("a", "data.bin", None, 10, 7)).unwrap();
    assert_eq!(Some(got.concat().as_slice()), bytes.get(10..17));
    assert_eq!((view.offset, view.sent, view.eof), (10, 7, false));
    let (got, view) = pieces(&cx, &read_req("a", "data.bin", None, 95, 99)).unwrap();
    assert_eq!(Some(got.concat().as_slice()), bytes.get(95..));
    assert_eq!((view.sent, view.eof, view.size), (5, true, 100));
}

#[test]
fn each_piece_is_read_from_disk_only_after_the_last_was_sent() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "big.bin", &vec![1u8; 3 * 64 * 1024]);
    let file = dir
        .path()
        .join("artifacts")
        .join(t.get().to_string())
        .join("v1/big.bin");
    let mut got: Vec<Vec<u8>> = Vec::new();
    read(
        &cx,
        &read_req("a", "big.bin", None, 0, u64::MAX),
        &mut |p| {
            if got.is_empty() {
                fs::write(&file, vec![2u8; 3 * 64 * 1024])?;
            }
            got.push(p.to_vec());
            Ok(())
        },
    )
    .unwrap();
    let firsts: Vec<Option<u8>> = got.iter().map(|p| p.first().copied()).collect();
    assert_eq!(firsts, [Some(1), Some(2), Some(2)]);
}

#[test]
fn a_receiver_that_fails_stops_the_read() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "big.bin", &vec![1u8; 3 * 64 * 1024]);
    let mut calls = 0;
    let e = read(
        &cx,
        &read_req("a", "big.bin", None, 0, u64::MAX),
        &mut |_| {
            calls += 1;
            Err(std::io::ErrorKind::BrokenPipe.into())
        },
    )
    .unwrap_err();
    assert_eq!((e.kind, calls), (ErrorKind::Failed, 1));
}

#[test]
fn a_named_version_is_read_not_the_latest() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    publish(&cx, t, "log.txt", b"two");
    let (got, view) = pieces(&cx, &read_req("a", "log.txt", Some(1), 0, 10)).unwrap();
    assert_eq!(got.concat(), b"one");
    assert_eq!(view.sha256, sha(b"one"));
}

#[test]
fn an_offset_at_the_end_sends_nothing_and_past_it_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"abc");
    let (got, view) = pieces(&cx, &read_req("a", "log.txt", None, 3, 5)).unwrap();
    assert!(got.is_empty() && view.eof && view.sent == 0);
    let e = pieces(&cx, &read_req("a", "log.txt", None, 4, 5)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}

#[test]
fn unknown_box_name_or_version_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let e = list(&cx, &list_req("nope", false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = pieces(&cx, &read_req("nope", "log.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = pieces(&cx, &read_req("a", "other.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = pieces(&cx, &read_req("a", "log.txt", Some(9), 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn a_missing_file_on_disk_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let file = dir
        .path()
        .join("artifacts")
        .join(t.get().to_string())
        .join("v1/log.txt");
    fs::remove_file(file).unwrap();
    let e = pieces(&cx, &read_req("a", "log.txt", None, 0, 1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn a_bad_name_is_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    new_box(&cx, "a");
    for bad in ["../x", "a/b", ""] {
        let e = pieces(&cx, &read_req("a", bad, None, 0, 1)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Usage, "{bad}");
    }
}

#[test]
fn a_symlinked_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
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
    let e = pieces(&cx, &read_req("a", "log.txt", None, 0, 99)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(!e.error.to_string().contains("s3cret"));
}

#[test]
fn a_symlinked_version_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    let box_dir = dir.path().join("artifacts").join(t.get().to_string());
    let moved = dir.path().join("elsewhere");
    fs::rename(box_dir.join("v1"), &moved).unwrap();
    symlink(&moved, box_dir.join("v1")).unwrap();
    let e = pieces(&cx, &read_req("a", "log.txt", None, 0, 99)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
}

mod target;
