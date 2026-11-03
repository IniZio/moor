//! The crabbox check.

use super::*;

const NODE: &str = "nodes:\n  local:\n    runtime: cloudhypervisor\n";

#[test]
fn reports_version_and_caps_of_each_nodes_provider() {
    let rig = Rig::new().with_config(NODE);
    let c = rig.with(|i| provider::check(i, &nodes::read(i)));
    assert_eq!(c.status, Status::Ok);
    assert_eq!(
        c.detail,
        "crabbox 0.79.1-moor.6; cloudhypervisor: sleep suspend, cache volumes yes"
    );
}

#[test]
fn fails_with_the_install_line_when_crabbox_is_missing() {
    let mut rig = Rig::new();
    rig.proc = Arc::new(ScriptedProc::new());
    let c = rig.with(|i| provider::check(i, &nodes::read(i)));
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("not on PATH"));
    assert!(c.fix.contains("docs/providers.md"));
}

#[test]
fn fails_on_an_upstream_crabbox() {
    let mut rig = Rig::new().with_config(NODE);
    rig.proc = Arc::new(ScriptedProc::new());
    rig.proc.on(
        Tool::Crabbox,
        &["--version"],
        Reply::Ok("crabbox 0.79.1\n".to_owned()),
    );
    let c = rig.with(|i| provider::check(i, &nodes::read(i)));
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("-moor"), "{c:?}");
}

#[test]
fn fails_when_crabbox_does_not_know_the_provider() {
    let rig = Rig::new().with_config("nodes:\n  x:\n    runtime: nosuch\n");
    let c = rig.with(|i| provider::check(i, &nodes::read(i)));
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("nosuch"), "{c:?}");
}

#[test]
fn passes_with_no_node_when_crabbox_runs() {
    let c = Rig::new().with(|i| provider::check(i, &nodes::read(i)));
    assert_eq!(c.status, Status::Ok);
    assert!(c.detail.starts_with("crabbox 0.79.1-moor.6"));
}
