//! Tests for secret file reading.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use super::read_secret;

/// Reads `path` as a token file.
fn read_token(path: &Path) -> Result<String, String> {
    read_secret(path, "token file")
}

/// Writes `body` to `dir/token` with `mode`.
fn token_file(dir: &Path, body: &str, mode: u32) -> std::path::PathBuf {
    let path = dir.join("token");
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn owner_only_file_is_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), "s3cret\n", 0o600);
    assert_eq!(read_token(&path).unwrap(), "s3cret");
}

#[test]
fn group_readable_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), "s3cret\n", 0o644);
    let err = read_token(&path).unwrap_err();
    assert!(err.contains("0600") && err.contains("token"), "{err}");
    assert!(!err.contains("s3cret"));
}

#[test]
fn empty_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), " \n", 0o600);
    assert!(read_token(&path).unwrap_err().contains("empty"));
}

#[test]
fn missing_file_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = read_token(&dir.path().join("nope")).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}
