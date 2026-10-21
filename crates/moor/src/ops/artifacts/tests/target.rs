//! `artifacts` and `artifact-read` by Box id next to the same requests by name.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::request::{ArtifactReadReq, ArtifactsReq};
use moor_core::{BoxId, BoxTarget, ErrorKind};

use super::{cx, name, new_box, pieces, publish, read_req, sha};
use crate::cx::Cx;
use crate::ops::artifacts::list;

fn close(cx: &Cx, id: BoxId) {
    let close = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    cx.store().unwrap().apply(close).unwrap();
}

/// The hash `artifacts` lists for `log.txt` and the bytes `artifact-read` sends for it.
fn log_of(cx: &Cx, target: &BoxTarget) -> (String, Vec<u8>) {
    let listed = list(
        cx,
        &ArtifactsReq {
            target: target.clone(),
            all: false,
        },
    )
    .unwrap();
    let req = ArtifactReadReq {
        target: target.clone(),
        ..read_req("a", "log.txt", None, 0, u64::MAX)
    };
    let (got, _) = pieces(cx, &req).unwrap();
    (listed.first().unwrap().sha256.clone(), got.concat())
}

#[test]
fn by_id_a_closed_box_is_listed_and_read_while_a_newer_box_holds_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let old = new_box(&cx, "a");
    publish(&cx, old, "log.txt", b"old");
    close(&cx, old);
    let newer = new_box(&cx, "a");
    publish(&cx, newer, "log.txt", b"new");
    assert_eq!(log_of(&cx, &old.into()), (sha(b"old"), b"old".to_vec()));
    assert_eq!(
        log_of(&cx, &name("a").into()),
        (sha(b"new"), b"new".to_vec())
    );
}

#[test]
fn by_name_a_closed_box_is_listed_and_read_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let id = new_box(&cx, "a");
    publish(&cx, id, "log.txt", b"kept");
    close(&cx, id);
    assert_eq!(
        log_of(&cx, &name("a").into()),
        (sha(b"kept"), b"kept".to_vec())
    );
}

#[test]
fn an_unknown_id_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path());
    let target: BoxTarget = BoxId::new(99).into();
    let listed = list(
        &cx,
        &ArtifactsReq {
            target: target.clone(),
            all: false,
        },
    )
    .unwrap_err();
    let req = ArtifactReadReq {
        target,
        ..read_req("a", "log.txt", None, 0, 1)
    };
    let read = pieces(&cx, &req).unwrap_err();
    for e in [listed, read] {
        assert_eq!(
            (e.kind, e.error.to_string()),
            (ErrorKind::NotFound, "no Box id 99".to_owned())
        );
    }
}
