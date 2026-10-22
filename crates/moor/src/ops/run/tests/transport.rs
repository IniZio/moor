//! A lost ssh connection while the user's command is launched.

use moor_core::seam::driver::DriverError;

use super::{ready_with, req};
use crate::ops::run::run;
use crate::testing::driver::FakeDriver;

#[test]
fn a_lost_connection_during_the_command_is_not_retried() {
    let f = ready_with(FakeDriver::new().fail_launch(DriverError::Transport {
        stderr_tail: "Connection reset by peer".into(),
    }));
    run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(f.driver.launches().len(), 1);
    let events = f.cx.store().unwrap().events(f.id, 1000).unwrap();
    let ran = events.iter().find(|e| e.kind == "ran").unwrap();
    assert_eq!(ran.payload.get("exit"), Some(&serde_json::Value::Null));
    assert_eq!(
        ran.payload.get("error").and_then(serde_json::Value::as_str),
        Some("moor lost the ssh connection to the Box: Connection reset by peer")
    );
}

#[test]
fn a_timeout_during_the_command_is_not_retried() {
    let f = ready_with(
        FakeDriver::new().fail_launch(DriverError::Timeout(std::time::Duration::from_secs(30))),
    );
    run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(f.driver.launches().len(), 1);
}
