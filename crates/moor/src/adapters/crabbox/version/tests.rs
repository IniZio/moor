use std::path::Path;
use std::sync::Arc;

use moor_core::domain::run::SyncMode;
use moor_core::name::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecSpec};
use moor_core::seam::proc::{Deadline, Tool};

use super::{check, Version};
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

fn accepted(out: &str) -> bool {
    check(out).is_ok()
}

#[test]
fn parses_moor_build_and_plain_releases() {
    let v = Version::parse("crabbox 0.72.0-moor.5\n").unwrap();
    assert_eq!(v.to_string(), "0.72.0-moor.5");
    assert_eq!(Version::parse("v0.73.1").unwrap().to_string(), "0.73.1");
    assert_eq!(Version::parse("0.72.0").unwrap().to_string(), "0.72.0");
}

#[test]
fn unreadable_versions_do_not_parse() {
    for bad in [
        "",
        "crabbox dev",
        "0.72",
        "0.72.0.1",
        "0.72.0-rc.1",
        "0.72.0-moor.x",
    ] {
        assert!(Version::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn compares_across_the_moor_suffix() {
    assert!(accepted("0.72.0-moor.7"));
    assert!(accepted("0.72.0-moor.8"));
    assert!(accepted("crabbox 0.73.0"));
    assert!(accepted("1.0.0"));
    assert!(!accepted("0.72.0-moor.6"));
    assert!(!accepted("0.72.0-moor.5"));
    assert!(!accepted("0.72.0"));
    assert!(!accepted("0.71.9-moor.9"));
}

#[test]
fn refusal_names_required_and_found_versions() {
    assert_eq!(
        check("crabbox 0.72.0-moor.6"),
        Err(DriverError::Failed {
            summary:
                "crabbox 0.72.0-moor.7 or newer is required, found 0.72.0-moor.6; upgrade crabbox"
                    .to_owned()
        })
    );
}

#[test]
fn unreadable_output_is_refused_with_the_text() {
    assert_eq!(
        check("garbage"),
        Err(DriverError::Failed {
            summary: "crabbox 0.72.0-moor.7 or newer is required, found \"garbage\" (unreadable); upgrade crabbox"
                .to_owned()
        })
    );
}

fn unchecked_fixture(version: Reply) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["--version"], version);
    p.on(Tool::Crabbox, &["providers"], Reply::Ok("[]".into()));
    let c = Crabbox::fake_unchecked(p.clone());
    (p, c)
}

#[test]
fn caps_refuses_an_old_crabbox_before_listing_providers() {
    let (p, c) = unchecked_fixture(Reply::Ok("crabbox 0.72.0-moor.6\n".into()));
    let err = c.caps(&ProviderId::new("sprites")).unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("0.72.0-moor.7") && text.contains("0.72.0-moor.6"));
    assert!(text.contains("upgrade"));
    assert_eq!(p.calls().len(), 1);
}

#[test]
fn caps_checks_the_version_once() {
    let (p, c) = unchecked_fixture(Reply::Ok("crabbox 0.72.0-moor.7\n".into()));
    let _ = c.caps(&ProviderId::new("sprites"));
    let _ = c.caps(&ProviderId::new("sprites"));
    let verbs: Vec<_> = p
        .calls()
        .into_iter()
        .filter_map(|c| c.args.into_iter().next())
        .collect();
    assert_eq!(verbs, ["--version", "providers", "providers"]);
}

type Op = fn(&Crabbox) -> bool;

fn handle() -> BoxHandle {
    BoxHandle::new("sprites/box")
}

fn path() -> BoxPath {
    BoxPath::restore("/w/f").unwrap()
}

fn ops() -> Vec<(&'static str, Op)> {
    vec![
        ("acquire", |c| {
            let req = AcquireReq {
                shape: moor_core::config::BoxShape::default(),
                provider: ProviderId::new("sprites"),
                task: TaskId::new(7),
                name: TaskName::parse("t7").unwrap(),
                class: None,
                nonce: "ab12cd".to_owned(),
                worktree: None,
            };
            c.acquire(&req).is_err()
        }),
        ("release", |c| c.release(&handle()).is_err()),
        ("list", |c| c.list(&ProviderId::new("sprites")).is_err()),
        ("workdir", |c| {
            c.workdir(&handle(), Path::new("/wt")).is_err()
        }),
        ("exec", |c| {
            let spec = ExecSpec {
                argv: vec!["true".to_owned()],
                env: Vec::new(),
                cwd: None,
                sync: SyncMode::None,
                output: None,
                host_cwd: None,
                class: None,
            };
            c.exec(
                &handle(),
                &spec,
                Deadline(std::time::Duration::from_secs(1)),
            )
            .is_err()
        }),
        ("copy_out", |c| {
            c.copy_out(&handle(), &path(), Path::new("/o"), Path::new("/wt"))
                .is_err()
        }),
        ("peek_file", |c| {
            c.peek_file(&handle(), &path(), Path::new("/wt")).is_err()
        }),
        ("put_file", |c| {
            c.put_file(&handle(), &path(), b"x", 0o600, Path::new("/wt"))
                .is_err()
        }),
        ("connect_argv", |c| {
            c.connect_argv(&handle(), Path::new("/wt")).is_err()
        }),
        ("reverse_tunnel", |c| {
            c.reverse_tunnel(&handle(), 1, 2, Path::new("/wt")).is_err()
        }),
        ("sleep", |c| c.sleep(&handle()).is_err()),
        ("wake", |c| c.wake(&handle()).is_err()),
    ]
}

#[test]
fn every_driver_call_refuses_an_old_crabbox_before_anything_else() {
    for (name, op) in ops() {
        let (p, c) = unchecked_fixture(Reply::Ok("crabbox 0.72.0-moor.6\n".into()));
        assert!(op(&c), "{name} was not refused");
        let verbs: Vec<_> = p
            .calls()
            .into_iter()
            .filter_map(|c| c.args.into_iter().next())
            .collect();
        assert_eq!(verbs, ["--version"], "{name}");
    }
}
