//! The `reason` values on a `closed` event that moor itself chose to close for.

/// A Box stayed in `setup_failed` past the grace period and the controller closed it.
pub const CLOSE_REASON_SETUP_FAILED_EXPIRED: &str = "setup_failed_expired";

/// A workflow run ended, and its Box saw no steer or terminal for the grace period after.
pub const CLOSE_REASON_RUN_ENDED: &str = "run_ended";

/// The Box's pull request merged.
pub const CLOSE_REASON_PR_MERGED: &str = "pr_merged";

/// The Box's pull request closed without a merge.
pub const CLOSE_REASON_PR_CLOSED: &str = "pr_closed";

/// A one-off Box saw no activity for the idle period.
pub const CLOSE_REASON_IDLE: &str = "idle";

/// `close-failed` closed the Box.
pub const CLOSE_REASON_CLOSE_FAILED: &str = "close_failed";
