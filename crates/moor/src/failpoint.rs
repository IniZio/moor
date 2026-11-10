//! Points where a build with the `failpoints` feature aborts when `MOOR_FAILPOINT` names one, so
//! a live check can stop the controller exactly between two inbox writes. A release build is
//! built without the feature, and each point is then a no-op.

/// The send of an inbox row is marked, and nothing is typed yet.
pub(crate) const INBOX_BEFORE_SEND: &str = "inbox_before_send";

/// The agent took the text of an inbox row, and its send is not confirmed yet.
pub(crate) const INBOX_AFTER_SEND: &str = "inbox_after_send";

/// Aborts the process when `MOOR_FAILPOINT` is `name`.
#[cfg(feature = "failpoints")]
pub(crate) fn hit(name: &str) {
    if std::env::var_os("MOOR_FAILPOINT").is_some_and(|v| v == name) {
        std::process::abort();
    }
}

/// Does nothing: this build has no failpoints.
#[cfg(not(feature = "failpoints"))]
pub(crate) const fn hit(_name: &str) {}
