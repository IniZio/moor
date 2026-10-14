use std::path::Path;
use std::sync::Arc;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;

use super::copy_out;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

fn fixture() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn run(c: &Crabbox) -> Result<(), DriverError> {
    copy_out(
        c,
        &BoxHandle::new("p/box"),
        &BoxPath::under("/work", "out/a.txt").unwrap(),
        Path::new("/host/a.txt"),
        Path::new("/host/cwd"),
    )
}

#[test]
fn argv_names_box_and_both_paths() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["cp"], Reply::Ok(String::new()));
    run(&c).unwrap();
    let calls = p.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls.first().map(|c| c.args.as_slice()),
        Some(
            &[
                "cp",
                "--provider",
                "p",
                "--id",
                "box",
                "SANDBOX:/work/out/a.txt",
                "/host/a.txt"
            ]
            .map(String::from)[..]
        )
    );
}

#[test]
fn runs_in_host_cwd() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["cp"], Reply::Ok(String::new()));
    run(&c).unwrap();
    assert_eq!(
        p.calls().first().and_then(|c| c.cwd.as_deref()),
        Some(Path::new("/host/cwd"))
    );
}

#[test]
fn passes_no_sync_arguments() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["cp"], Reply::Ok(String::new()));
    run(&c).unwrap();
    let calls = p.calls();
    let args = calls.first().map(|c| c.args.as_slice()).unwrap_or_default();
    assert!(!args.is_empty());
    assert!(args.iter().all(|a| !a.contains("sync") && a != "--hydrate"));
}

#[test]
fn failure_hides_raw_stderr() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["cp"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "secret raw text".to_owned(),
        },
    );
    let err = run(&c).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    assert!(!format!("{err:?}").contains("secret raw text"), "{err:?}");
}
