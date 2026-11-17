//! Pausing a lease: its port tunnels stop first, and stay refused until the lease is up again.

use moor_client::link::lease::Answer;

use super::{port, Crabbox, CALL};

/// Pauses the lease.
pub(super) async fn pause(lease: &Crabbox<'_>) -> Result<Answer, String> {
    let slug = lease.slug.unwrap_or_default();
    port::stop_lease(slug).await;
    let paused = match lease.run_with("pause", &["--id"], CALL, &[0], &[]).await {
        Ok(_) => Ok(Answer::Done),
        Err(e) if e.contains("does not support pause") => Ok(Answer::Unsupported),
        Err(e) => Err(e),
    };
    if !matches!(paused, Ok(Answer::Done)) {
        port::allow(slug).await;
    }
    paused
}
