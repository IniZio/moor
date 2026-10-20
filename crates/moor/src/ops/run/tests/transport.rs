//! A lost ssh connection during the user's command.

use moor_core::seam::driver::DriverError;

use super::stage::{command_execs, command_results};
use super::{ready_with, req};
use crate::ops::run::run;

#[test]
fn a_lost_connection_during_the_command_is_not_retried() {
    let f = ready_with(command_results([Err(DriverError::Transport {
        stderr_tail: "Connection reset by peer".into(),
    })]));
    run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(command_execs(&f).len(), 1);
    let events = f.cx.store().unwrap().events(f.id, 1000).unwrap();
    let ran = events.iter().find(|e| e.kind == "ran").unwrap();
    assert_eq!(ran.payload.get("exit"), Some(&serde_json::Value::Null));
    assert_eq!(
        ran.payload.get("error").and_then(serde_json::Value::as_str),
        Some("moor lost the ssh connection to the Box: Connection reset by peer")
    );
}
