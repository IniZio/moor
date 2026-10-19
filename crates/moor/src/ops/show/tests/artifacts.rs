//! Tests for the artifacts section of `show`.

use std::sync::Arc;

use super::super::run;
use super::{add, cx, req};
use crate::testing::driver::FakeDriver;

#[test]
fn show_adds_the_latest_artifacts_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let bytes = b"png bytes";
    cx.store()
        .unwrap()
        .add_artifact(id, "shot.png", bytes, &dir.path().join("artifacts"))
        .unwrap();
    let mut asked = req(Some("a"), None);
    asked.artifacts = true;
    let shown = run(&cx, &asked).unwrap().artifacts.unwrap();
    assert_eq!(shown.len(), 1);
    let art = shown.first().unwrap();
    assert_eq!(
        (art.name.as_str(), art.version, art.content_type.as_str()),
        ("shot.png", 1, "image/png")
    );
    assert_eq!(art.size, bytes.len() as u64);
    assert_eq!(art.sha256.len(), 64);
    assert!(run(&cx, &req(Some("a"), None)).unwrap().artifacts.is_none());
    assert!(run(&cx, &req(None, None)).unwrap().artifacts.is_none());
}
