//! How long each crabbox call may take.

use std::time::Duration;

use super::Crabbox;

/// Time allowed for a warmup.
pub(super) const ACQUIRE_DEADLINE: Duration = Duration::from_mins(15);
/// Time allowed for a provider inventory listing.
pub(super) const LIST_DEADLINE: Duration = Duration::from_secs(5);
/// Time allowed for every other call.
pub(super) const QUICK_DEADLINE: Duration = Duration::from_secs(60);
/// Time allowed for an ssh target lookup unless the provider has its own.
pub(super) const LOOKUP_DEADLINE: Duration = Duration::from_secs(5);
/// An external provider's ssh lookup runs through the adapter and was measured at ~6 s.
pub(super) const EXTERNAL_LOOKUP_DEADLINE: Duration = Duration::from_secs(20);

impl Crabbox {
    /// Time allowed for an ssh target lookup on `provider`.
    pub(super) fn lookup_deadline(&self, provider: &str) -> Duration {
        let built_in = if provider == "external" {
            EXTERNAL_LOOKUP_DEADLINE
        } else {
            LOOKUP_DEADLINE
        };
        *self.lookup_deadlines.get(provider).unwrap_or(&built_in)
    }
}
