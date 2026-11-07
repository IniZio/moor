//! Ending a lease, and taking over one whose boot a cut-off caller left going.

use moor_client::link::lease::{Answer, Boot};

use super::{box_dir, flight, list, warmup, Crabbox, CALL, GONE};

/// Deletes lease `slug` and its dir, with no boot of it going on.
pub(super) async fn stop_now(lease: &Crabbox<'_>, slug: &str) -> Result<String, String> {
    let gone = lease.run("stop", "--id", CALL, &[0, GONE]).await;
    let _ = tokio::fs::remove_dir_all(box_dir(lease.home, slug)).await;
    gone
}

/// Takes over lease `slug` for a caller whose earlier warmup or fork was cut off: waits for one
/// going on, or finds its VM up, or boots the VM.
pub(super) async fn adopt(lease: &Crabbox<'_>, slug: &str, boot: &Boot) -> Result<Answer, String> {
    if let Some(ended) = flight::join(&box_dir(lease.home, slug)).await {
        return ended.map(|()| Answer::Done);
    }
    let up = list(lease.home, lease.pool).await?;
    if up.iter().any(|l| l.slug == slug) {
        return Ok(Answer::Done);
    }
    warmup(lease, slug, boot).await.map(|_| Answer::Done)
}
