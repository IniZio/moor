use std::path::Path;
use std::sync::Arc;

use moor_core::name::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};
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
fn accepts_moor_builds_from_the_required_one() {
    assert!(accepted("0.79.1-moor.1"));
    assert!(accepted("0.79.1-moor.10"));
    assert!(accepted("crabbox 0.80.0-moor.1"));
    assert!(!accepted("0.79.1-moor.0"));
    assert!(!accepted("0.79.0-moor.9"));
    assert!(!accepted("0.72.0-moor.9"));
}

#[test]
fn refuses_upstream_releases_however_new() {
    for upstream in ["0.72.0", "crabbox 0.73.0", "1.0.0"] {
        assert!(!accepted(upstream), "{upstream}");
    }
}

/// The refusal for `found`.
fn refusal(found: &str) -> Result<(), DriverError> {
    Err(DriverError::Failed {
        summary: format!(
            "crabbox 0.79.1-moor.1 or a newer -moor build of IniZio/crabbox is required, \
             found {found}; install it"
        ),
    })
}

#[test]
fn refusal_names_required_and_found_versions() {
    assert_eq!(check("crabbox 0.72.0-moor.6"), refusal("0.72.0-moor.6"));
    assert_eq!(check("crabbox 0.73.0"), refusal("0.73.0"));
}

#[test]
fn unreadable_output_is_refused_with_the_text() {
    assert_eq!(check("garbage"), refusal("\"garbage\" (unreadable)"));
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
    assert!(text.contains("0.79.1-moor.1") && text.contains("0.72.0-moor.6"));
    assert!(text.contains("IniZio/crabbox"));
    assert_eq!(p.calls().len(), 1);
}

#[test]
fn caps_checks_the_version_once() {
    let (p, c) = unchecked_fixture(Reply::Ok("crabbox 0.79.1-moor.1\n".into()));
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
                provider: ProviderId::new("sprites"),
                box_id: BoxId::new(7),
                name: BoxName::parse_allow_bare("t7").unwrap(),
                class: None,
                nonce: "ab12cd".to_owned(),
            };
            c.acquire(&req).is_err()
        }),
        ("release", |c| c.release(&handle()).is_err()),
        ("list", |c| c.list(&ProviderId::new("sprites")).is_err()),
        ("exec", |c| {
            let spec = ExecSpec {
                argv: vec!["true".to_owned()],
                env: Vec::new(),
                cwd: None,
                output: None,
            };
            c.exec(
                &handle(),
                &spec,
                Deadline(std::time::Duration::from_secs(1)),
            )
            .is_err()
        }),
        ("copy_out", |c| {
            c.copy_out(&handle(), &path(), Path::new("/o")).is_err()
        }),
        ("peek_file", |c| c.peek_file(&handle(), &path()).is_err()),
        ("put_file", |c| {
            c.put_file(&handle(), &path(), b"x", 0o600).is_err()
        }),
        ("connect_argv", |c| c.connect_argv(&handle()).is_err()),
        ("reverse_tunnel", |c| {
            c.reverse_tunnel(&handle(), 1, Path::new("/s")).is_err()
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
