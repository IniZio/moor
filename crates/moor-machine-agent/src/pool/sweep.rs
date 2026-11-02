//! Deleting the leases whose Box the controller no longer has.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::ops::Reply;
use moor_client::link::sweep::{Failed, Swept};
use moor_core::box_key::parse_pool_slug;

use super::{box_dir, lease, list, Crabbox, Pool, CALL};

/// Stops each lease of the pool whose Box is not in `keep` and whose dir is older than `grace`,
/// whatever its state, then has crabbox clear what stopped leases leave behind.
///
/// The runner lists the open Boxes before the pool is looked at, so a lease booted in between
/// is missing from `keep`; its dir is younger than the grace, which is what spares it.
pub(crate) async fn sweep(home: &Path, pool: &Pool, keep: &[i64], grace: Duration) -> Swept {
    let mut swept = Swept::default();
    let leases = match list(home, pool).await {
        Ok(leases) => leases,
        Err(reason) => {
            swept.errors.push(Failed { box_id: 0, reason });
            return swept;
        }
    };
    let since =
        crate::unix_now().saturating_sub(i64::try_from(grace.as_secs()).unwrap_or(i64::MAX));
    for found in leases {
        let Some((_, id)) = parse_pool_slug(&found.slug) else {
            continue;
        };
        let id = id.get();
        // A lease always has its dir, made before it boots; one without is long past the grace.
        let young =
            std::fs::symlink_metadata(box_dir(home, &found.slug)).is_ok_and(|m| m.ctime() > since);
        if keep.contains(&id) || young {
            continue;
        }
        match lease(
            home,
            pool,
            Verb::Stop,
            Some(&found.slug),
            None,
            &Boot::default(),
        )
        .await
        {
            Reply::Leased(Answer::Done) => swept.removed.push(id),
            Reply::Refused { reason } => swept.errors.push(Failed { box_id: id, reason }),
            other => swept.errors.push(Failed {
                box_id: id,
                reason: format!("unexpected answer to stop: {other:?}"),
            }),
        }
    }
    let cleanup = Crabbox {
        home,
        pool,
        slug: None,
    };
    // Best effort: the next sweep runs it again.
    let _ = cleanup.run_with("cleanup", &[], CALL, &[0], &[]).await;
    swept
}
