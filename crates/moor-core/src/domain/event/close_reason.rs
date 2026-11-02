//! The `reason` values on a `closed` event that moor itself chose to close for.

/// A Box stayed in `setup_failed` past the grace period and the controller closed it.
pub const CLOSE_REASON_SETUP_FAILED_EXPIRED: &str = "setup_failed_expired";

/// `close-failed` closed the Box.
pub const CLOSE_REASON_CLOSE_FAILED: &str = "close_failed";
