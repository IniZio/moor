//! `box-put` against the fake Driver.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxId, BoxName, BoxPutReq, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

/// Box `t`, inserted and admitted; `ready` also gives it a live machine.
fn fx_with(driver: FakeDriver, ready: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let d = if ready {
        let fields = Fields {
            box_provider: Field::Set(provider.clone()),
            ..Fields::default()
        };
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(fields)
    } else {
        Decision::new(id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 })
    };
    store.apply(d).unwrap();
    if ready {
        let got = driver
            .acquire(&AcquireReq {
                pool_machine: None,
                provider,
                box_id: id,
                name,
                class: None,
                nonce: "n".to_owned(),
                cache: moor_core::cache::CachePlan::none(),
            })
            .unwrap();
        let d = Decision::new(id)
            .lifecycle(BoxLifecycleInput::Ready {
                setup: SetupOutcome::Ok,
            })
            .session(SessionInput::Started)
            .box_input(BoxInput::Acquired { handle: got.handle });
        store.apply(d).unwrap();
    }
    Fx {
        dir,
        cx,
        driver,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new(), true)
}

fn req(f: &Fx, src: &std::path::Path, dest: &str) -> BoxPutReq {
    BoxPutReq {
        id: f.id,
        src: src.display().to_string(),
        dest: dest.to_owned(),
    }
}

fn file(f: &Fx, bytes: &[u8]) -> std::path::PathBuf {
    let p = f.dir.path().join("src.bin");
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn a_bad_destination_is_refused_naming_it_before_the_driver_is_called() {
    let f = fx();
    let src = file(&f, b"x");
    for bad in ["", "/etc/passwd", "../up", "a/../../up"] {
        let err = run(&f.cx, &req(&f, &src, bad)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{bad:?}");
        assert!(
            err.error.to_string().contains(&format!("{bad:?}")),
            "{err:?}"
        );
    }
    assert!(f.driver.puts().is_empty());
}

#[test]
fn a_missing_source_is_refused() {
    let f = fx();
    let gone = f.dir.path().join("gone");
    let err = run(&f.cx, &req(&f, &gone, "a.png")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.error.to_string().contains("cannot read"), "{err:?}");
    assert!(f.driver.puts().is_empty());
}

#[test]
fn a_directory_source_is_refused() {
    let f = fx();
    let err = run(&f.cx, &req(&f, f.dir.path(), "a.png")).unwrap_err();
    assert!(
        err.error.to_string().contains("not a regular file"),
        "{err:?}"
    );
    assert!(f.driver.puts().is_empty());
}

#[test]
fn an_oversize_source_is_refused_naming_its_size() {
    let f = fx();
    let src = f.dir.path().join("big");
    let big = std::fs::File::create(&src).unwrap();
    big.set_len(32 * 1024 * 1024 + 1).unwrap();
    let err = run(&f.cx, &req(&f, &src, "big.bin")).unwrap_err();
    assert!(err.error.to_string().contains("33554433 bytes"), "{err:?}");
    assert!(f.driver.puts().is_empty());
}

#[test]
fn a_relative_source_is_usage() {
    let f = fx();
    let err = run(&f.cx, &req(&f, std::path::Path::new("rel"), "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn an_unknown_box_id_is_refused() {
    let f = fx();
    let src = file(&f, b"x");
    let mut r = req(&f, &src, "a");
    r.id = BoxId::new(f.id.get() + 1);
    let err = run(&f.cx, &r).unwrap_err();
    assert!(err.error.to_string().contains("no open Box"), "{err:?}");
}

#[test]
fn a_good_request_writes_the_bytes_under_the_work_tree_at_0644() {
    let f = fx();
    let src = file(&f, b"png bytes");
    let got = run(&f.cx, &req(&f, &src, "shots/1.png")).unwrap();
    let puts = f.driver.puts();
    assert_eq!(puts.len(), 1);
    let (_, path, bytes, mode) = puts.first().unwrap();
    assert!(
        path.as_str().ends_with("/repo/shots/1.png"),
        "{}",
        path.as_str()
    );
    assert_eq!(bytes, b"png bytes");
    assert_eq!(*mode, 0o644);
    assert_eq!(got.path, path.as_str());
    assert_eq!(got.bytes, 9);
}

#[test]
fn a_source_held_to_a_host_directory_is_read_only_beneath_it() {
    let f = fx();
    let outside = file(&f, b"secret");
    let root = f.dir.path().join("runner");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("staged"), b"staged").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    run(&f.cx, &req(&f, &root.join("link"), "a")).expect("the operator follows the link");
    let f = Fx {
        cx: f.cx.with_grant(moor_core::request::Grant {
            host_dir: Some(root.clone()),
            ..moor_core::request::Grant::operator()
        }),
        ..f
    };
    let e = run(&f.cx, &req(&f, &root.join("link"), "b")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{}", e.error);
    run(&f.cx, &req(&f, &outside, "c")).unwrap_err();
    run(&f.cx, &req(&f, &root.join("staged"), "d")).expect("inside");
    assert_eq!(f.driver.puts().last().unwrap().2, b"staged");
}
