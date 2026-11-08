//! A Box's first op waits until its new machine's guest answers.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::{Driver as _, DriverError};
use moor_core::seam::proxy_host::{
    ProxyEndpoint, ProxyHost, ProxyHostError, ProxySpec, ProxyStatus,
};
use moor_core::{BoxId, BoxPath};

use super::setup::{kinds, start_req};
use super::{script_base_yaml, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proxy_host::FakeProxyHost;

const STALLED: DriverError = DriverError::Timeout(Duration::from_secs(5));

fn environ() -> BoxPath {
    BoxPath::restore("/proc/self/environ").unwrap()
}

/// A Proxy host that reads the Box user's home before it starts, as the controller's does.
struct ReadsHome {
    host: FakeProxyHost,
    driver: Arc<FakeDriver>,
}

impl ProxyHost for ReadsHome {
    fn ensure(&self, spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError> {
        let handle = spec.box_handle.as_ref().expect("a Box to deliver to");
        self.driver
            .peek_file(handle, &environ())
            .map_err(|e| ProxyHostError::Failed(format!("cannot find the Box user's home: {e}")))?;
        self.host.ensure(spec)
    }
    fn restart(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        self.host.restart(box_id)
    }
    fn status(&self, box_id: BoxId) -> Result<ProxyStatus, ProxyHostError> {
        self.host.status(box_id)
    }
    fn front_running(&self, front: &str) -> bool {
        self.host.front_running(front)
    }
    fn front_ca_pem(&self) -> Result<String, String> {
        self.host.front_ca_pem()
    }
    fn stop(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        self.host.stop(box_id)
    }
    fn park(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        self.host.park(box_id)
    }
    fn restore(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        self.host.restore(box_id)
    }
}

fn world() -> World {
    let mut w = super::world(FakeDriver::new());
    let host = ReadsHome {
        host: FakeProxyHost::new(),
        driver: w.driver.clone(),
    };
    w.cx = w.cx.with_proxy_host(Arc::new(host));
    script_base_yaml(&w, Some("setup: [a]\n"));
    w
}

fn lifecycle(w: &World) -> BoxLifecycle {
    let store = w.cx.store().unwrap();
    store.box_row(BoxId::new(1)).unwrap().unwrap().lifecycle
}

#[test]
fn setup_waits_for_a_new_guest_that_answers_late() {
    let w = world();
    w.driver.silent_peeks(vec![STALLED, STALLED]);
    run(&w.cx, &start_req()).unwrap();
    let kinds = kinds(&w, BoxId::new(1));
    assert!(!kinds.contains(&"setup_failed".to_owned()), "{kinds:?}");
    assert!(
        !kinds.contains(&"agent_unreachable".to_owned()),
        "{kinds:?}"
    );
    assert_ne!(lifecycle(&w), BoxLifecycle::SetupFailed);
}

#[test]
fn a_new_guest_that_never_answers_is_waited_for_within_the_bound() {
    let w = world();
    w.driver.fail_peek_of(&environ(), STALLED);
    let from = w.cx.clock().now().as_millis();
    let e = run(&w.cx, &start_req()).unwrap_err();
    assert!(
        e.error
            .to_string()
            .contains("cannot find the Box user's home"),
        "{}",
        e.error
    );
    let took = w.cx.clock().now().as_millis() - from;
    assert!((120_000..=125_000).contains(&took), "{took}");
    let kinds = kinds(&w, BoxId::new(1));
    assert!(kinds.contains(&"agent_unreachable".to_owned()), "{kinds:?}");
    assert_eq!(lifecycle(&w), BoxLifecycle::SetupFailed);
}
