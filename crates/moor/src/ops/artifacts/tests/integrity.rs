//! `artifact-read` checks the stored bytes against the recorded sha256 and names missing bytes.

use std::fs;
use std::path::{Path, PathBuf};

use moor_core::response::ErrorDetail;
use moor_core::{BoxId, ErrorKind};

use super::{cx, new_box, pieces, publish, read_req};
use crate::ops::artifacts::read;

fn stored(dir: &Path, id: BoxId) -> PathBuf {
    dir.join("artifacts")
        .join(id.get().to_string())
        .join("v1/log.txt")
}

#[test]
fn a_whole_read_of_bytes_that_do_not_match_the_record_is_refused_before_sending() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    fs::write(stored(dir.path(), t), b"two").unwrap();
    let mut sent = 0;
    let e = read(&cx, &read_req("a", "log.txt", None, 0, 99), &mut |_| {
        sent += 1;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert_eq!(e.detail.as_deref(), Some(&ErrorDetail::ArtifactMismatch));
    assert!(e.error.to_string().contains("sha256"), "{}", e.error);
    assert_eq!(sent, 0);
}

#[test]
fn a_range_read_is_served_without_the_whole_file_check() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    fs::write(stored(dir.path(), t), b"two").unwrap();
    let (got, _) = pieces(&cx, &read_req("a", "log.txt", None, 1, 1)).unwrap();
    assert_eq!(got.concat(), b"w");
}

#[test]
fn a_missing_file_carries_the_missing_detail() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let t = new_box(&cx, "a");
    publish(&cx, t, "log.txt", b"one");
    fs::remove_file(stored(dir.path(), t)).unwrap();
    let e = pieces(&cx, &read_req("a", "log.txt", None, 0, 99)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    assert_eq!(e.detail.as_deref(), Some(&ErrorDetail::ArtifactMissing));
}
