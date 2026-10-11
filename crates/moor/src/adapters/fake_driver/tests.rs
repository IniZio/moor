//! Self-tests for the Driver fake.

use std::sync::Arc;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan, Released};
use moor_core::{BoxHandle, ProviderId, TaskId};

use super::FakeDriver;

fn req() -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        egress: EgressPlan::Open,
        class: None,
    }
}

#[test]
fn shared_arc_sees_calls_and_vanished_box_is_already_gone() {
    let fake = Arc::new(FakeDriver::new());
    let driver: Arc<dyn Driver> = fake.clone();
    let got = driver.acquire(&req());
    assert_eq!(got.as_ref().map(|a| a.handle.as_str()), Ok("box-1"));
    fake.forget(&BoxHandle::new("box-1"));
    assert_eq!(
        driver.release(&BoxHandle::new("box-1")),
        Ok(Released::AlreadyGone)
    );
    assert_eq!(fake.calls(), ["acquire", "release box-1"]);
}

#[test]
fn scripted_acquire_failure_is_one_shot() {
    let fake = FakeDriver::new().fail_acquire(DriverError::Quota("q".into()));
    assert_eq!(fake.acquire(&req()), Err(DriverError::Quota("q".into())));
    assert!(fake.acquire(&req()).is_ok());
}
