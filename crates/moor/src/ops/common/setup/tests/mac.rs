//! Setup on a macOS Task layout without a Proxy.

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::domain::run::SyncMode;
use moor_core::{BoxPath, TaskId, TaskName};

use super::{cfg, rig, run_setup_with};
use crate::testing::driver::FakeDriver;

const WORKDIR: &str = "/Users/box/moor/demo/work/repo";

#[test]
fn setup_without_a_proxy_on_a_mac_layout_gets_the_task_env() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    r.cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    let workdir = BoxPath::restore(WORKDIR).unwrap();
    run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
        None,
        TaskId::new(1),
    );
    let execs = r.driver.execs();
    let (_, setup, _) = execs.last().expect("a setup exec");
    assert_eq!(execs.len(), 2, "layout prepare, then setup");
    let var = |name: &str| {
        setup
            .env
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(var("HOME"), Some("/Users/box/moor/demo/home"));
    assert!(
        var("MOOR_BOX_TAG").is_some_and(|t| t.starts_with("demo-")),
        "{:?}",
        setup.env
    );
    assert!(!r.driver.calls().iter().any(|c| c.starts_with("peek_file")));
}
