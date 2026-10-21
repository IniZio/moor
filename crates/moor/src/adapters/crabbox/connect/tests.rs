use std::sync::Arc;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

const SSH_LINE: &str = "ssh -i '/k/id' -p '22' 'u@10.0.0.1'\n";

fn connect(p: &Arc<ScriptedProc>) -> Result<Vec<String>, DriverError> {
    Crabbox::fake(p.clone()).connect_argv(&BoxHandle::new("p/box"))
}

#[test]
fn argv_is_ssh_with_a_tty_then_the_looked_up_target() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    assert_eq!(
        connect(&p).unwrap(),
        ["ssh", "-t", "-i", "/k/id", "-p", "22", "u@10.0.0.1"]
    );
}

#[test]
fn lookup_uses_crabbox_ssh_only() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()));
    connect(&p).unwrap();
    let args: Vec<_> = p.calls().into_iter().map(|c| c.args).collect();
    assert_eq!(args, [["ssh", "--provider", "p", "--id", "box"]]);
}

#[test]
fn gone_box_is_not_found() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Fail {
            code: 4,
            stdout: String::new(),
            stderr: "lease not found".to_owned(),
        },
    );
    assert_eq!(connect(&p), Err(DriverError::NotFound));
}

#[test]
fn unreadable_target_line_is_an_error() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["ssh"], Reply::Ok("not ssh\n".to_owned()));
    assert!(matches!(connect(&p), Err(DriverError::Failed { .. })));
}
