//! Helpers shared by the task ops.

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "no op reads agent activity yet")
)]
pub(crate) mod activity;
pub(crate) mod boxlock;
pub(crate) mod capacity;
pub(crate) mod creds;
pub(crate) mod facts;
pub(crate) mod identity;
pub(crate) mod promote;
pub(crate) mod provision;
pub(crate) mod provisioning;
pub(crate) mod reconcile;
pub(crate) mod setup;
pub(crate) mod view;
pub(crate) mod wake;
