//! Tests for the artifact manifest writer.

use std::os::unix::fs::{symlink, PermissionsExt};

use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use super::{write_manifest, ArtifactRow};

fn row(name: &str, version: i64, kind: &str, ct: &str, created_ms: i64) -> ArtifactRow {
    ArtifactRow {
        id: version,
        task: BoxId::new(1),
        session: None,
        name: name.to_owned(),
        version,
        kind: kind.to_owned(),
        content_type: ct.to_owned(),
        size: 123,
        sha256: "ab".repeat(32),
        path: format!("/box/{name}"),
        created_at: Ts::from_millis(created_ms),
    }
}

#[test]
fn writes_exact_json_for_two_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rows = [
        row("log.txt", 1, "log", "text/plain", 1_700_000_000_000),
        row("shot.png", 2, "screenshot", "image/png", 1_700_000_060_000),
    ];
    write_manifest(dir.path(), &rows, Ts::from_millis(1_700_000_120_000)).expect("write");
    let got = std::fs::read_to_string(dir.path().join("artifact-manifest.json")).expect("read");
    let sha = "ab".repeat(32);
    let expected = format!(
        r#"{{
  "schemaVersion": 1,
  "generatedAt": "2023-11-14T22:15:20Z",
  "storage": {{
    "backend": "local"
  }},
  "files": [
    {{
      "kind": "log",
      "name": "log.txt",
      "path": "/box/log.txt",
      "contentType": "text/plain",
      "size": 123,
      "sha256": "{sha}",
      "version": 1,
      "publishedAt": "2023-11-14T22:13:20Z"
    }},
    {{
      "kind": "screenshot",
      "name": "shot.png",
      "path": "/box/shot.png",
      "contentType": "image/png",
      "size": 123,
      "sha256": "{sha}",
      "version": 2,
      "publishedAt": "2023-11-14T22:14:20Z"
    }}
  ]
}}
"#
    );
    assert_eq!(got, expected);
}

#[test]
fn empty_rows_give_empty_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_manifest(dir.path(), &[], Ts::from_millis(0)).expect("write");
    let got = std::fs::read_to_string(dir.path().join("artifact-manifest.json")).expect("read");
    let v: serde_json::Value = serde_json::from_str(&got).expect("json");
    assert_eq!(v.get("files"), Some(&serde_json::json!([])));
    assert_eq!(v.get("schemaVersion"), Some(&serde_json::json!(1)));
}

#[test]
fn file_mode_is_private() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_manifest(dir.path(), &[], Ts::from_millis(0)).expect("write");
    let meta = std::fs::metadata(dir.path().join("artifact-manifest.json")).expect("meta");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn overwrite_replaces_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let one = [row("a.txt", 1, "log", "text/plain", 0)];
    write_manifest(dir.path(), &one, Ts::from_millis(0)).expect("first");
    write_manifest(dir.path(), &[], Ts::from_millis(0)).expect("second");
    let got = std::fs::read_to_string(dir.path().join("artifact-manifest.json")).expect("read");
    assert!(!got.contains("a.txt"));
    assert!(!dir.path().join(".artifact-manifest.json.tmp").exists());
}

#[test]
fn symlink_at_target_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let victim = dir.path().join("victim");
    std::fs::write(&victim, "keep").expect("victim");
    symlink(&victim, dir.path().join("artifact-manifest.json")).expect("link");
    let err = write_manifest(dir.path(), &[], Ts::from_millis(0)).expect_err("refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(std::fs::read_to_string(&victim).expect("read"), "keep");
}

#[test]
fn symlink_at_temp_path_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let victim = dir.path().join("victim");
    std::fs::write(&victim, "keep").expect("victim");
    symlink(&victim, dir.path().join(".artifact-manifest.json.tmp")).expect("link");
    write_manifest(dir.path(), &[], Ts::from_millis(0)).expect_err("refused");
    assert_eq!(std::fs::read_to_string(&victim).expect("read"), "keep");
}
