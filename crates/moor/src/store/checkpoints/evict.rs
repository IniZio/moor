//! Which setup checkpoints may go to free disk, in the order they go, and the Boxes that hold
//! some back.

use moor_core::domain::layers::LayerKey;
use moor_core::{BoxId, ProviderId};
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

/// Why a checkpoint goes before the ones after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvictReason {
    /// The same user began a chain of the same repo after this one's, at another commit.
    Superseded,
    /// No newer chain replaces it; it is the one used longest ago.
    LeastRecentlyUsed,
}

impl EvictReason {
    /// The name events carry.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Superseded => "superseded",
            Self::LeastRecentlyUsed => "least_recently_used",
        }
    }
}

/// A checkpoint no Box holds back and no other checkpoint is a delta over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evictable {
    /// Its layer's key.
    pub(crate) layer_key: String,
    /// The provider's checkpoint id.
    pub(crate) checkpoint: String,
    /// The repo whose setup it holds.
    pub(crate) repo: String,
    /// Its size on the host, when reported.
    pub(crate) bytes: Option<u64>,
    /// Why it goes where it does in the order.
    pub(crate) reason: EvictReason,
}

/// True for a checkpoint `c` that a Box holds back: one setting up or waiting to, which may be
/// forking it, and one whose setup failed, for its deepest checkpoint, where a retry resumes.
const HELD: &str = "EXISTS (SELECT 1 FROM setup_checkpoint_pins p JOIN boxes b ON b.id = p.box_id \
     WHERE p.provider = c.provider AND p.layer_key = c.layer_key \
     AND (b.state IN ('pending', 'setting_up') OR (b.state = 'setup_failed' \
     AND p.layer = (SELECT MAX(q.layer) FROM setup_checkpoint_pins q \
     WHERE q.box_id = p.box_id AND q.provider = p.provider))))";

/// True for a checkpoint `c` that another checkpoint kept on the disk is a delta over, which
/// the provider refuses to delete first.
const PARENT: &str = "EXISTS (SELECT 1 FROM setup_checkpoints d \
     WHERE d.provider = c.provider AND d.parent = c.checkpoint)";

/// True for a checkpoint `c` whose user began another chain of the same repo after `c`'s
/// chain began, as a Box at a newer commit does.
const SUPERSEDED: &str = "EXISTS (SELECT 1 FROM setup_checkpoints n \
     WHERE n.provider = c.provider AND n.user = c.user AND n.repo = c.repo \
     AND n.chain <> c.chain AND n.chain_at > c.chain_at)";

impl Store {
    /// Marks layer `layer`, keyed `key`, under `provider` as one Box `id` stands on.
    pub(crate) fn pin_checkpoint(
        &self,
        id: BoxId,
        provider: &ProviderId,
        key: &LayerKey,
        layer: usize,
    ) -> Result<(), StoreError> {
        self.writer().execute(
            "INSERT OR IGNORE INTO setup_checkpoint_pins (box_id, provider, layer_key, layer) \
             VALUES (?, ?, ?, ?)",
            params![
                id.get(),
                provider.as_str(),
                key.as_str(),
                i64::try_from(layer).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(())
    }

    /// The checkpoints under `provider` that may be claimed for deleting: no Box holds them back
    /// and none is a delta over them; superseded chains first, then the least recently used.
    pub(crate) fn evictable(&self, provider: &ProviderId) -> Result<Vec<Evictable>, StoreError> {
        self.to_evict(provider, &format!("c.evicting = 0 AND NOT {HELD}"))
    }

    /// The checkpoints under `provider` claimed for deleting and still tracked, as a deleter cut
    /// short leaves them.
    pub(crate) fn evicting(&self, provider: &ProviderId) -> Result<Vec<Evictable>, StoreError> {
        self.to_evict(provider, "c.evicting = 1")
    }

    /// The leaf checkpoints under `provider` that `filter` keeps, in the order they go.
    fn to_evict(&self, provider: &ProviderId, filter: &str) -> Result<Vec<Evictable>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT c.layer_key, c.checkpoint, c.repo, c.bytes, {SUPERSEDED} \
             FROM setup_checkpoints c WHERE c.provider = ? AND {filter} AND NOT {PARENT} \
             ORDER BY 5 DESC, c.last_used_at, c.created_at"
        ))?;
        let rows = stmt.query_map([provider.as_str()], |r| {
            Ok(Evictable {
                layer_key: r.get(0)?,
                checkpoint: r.get(1)?,
                repo: r.get(2)?,
                bytes: r
                    .get::<_, Option<i64>>(3)?
                    .and_then(|b| u64::try_from(b).ok()),
                reason: if r.get(4)? {
                    EvictReason::Superseded
                } else {
                    EvictReason::LeastRecentlyUsed
                },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Marks `gone` as being deleted, unless a Box began holding it back, a delta over it was
    /// saved, or it is marked already; returns whether it was marked, and so may be deleted.
    /// A marked checkpoint stays tracked, but no Box starts from it.
    pub(crate) fn claim_checkpoint(
        &self,
        provider: &ProviderId,
        gone: &Evictable,
    ) -> Result<bool, StoreError> {
        let changed = self.writer().execute(
            &format!(
                "UPDATE setup_checkpoints AS c SET evicting = 1 \
                 WHERE c.provider = ? AND c.checkpoint = ? AND c.evicting = 0 \
                 AND NOT {HELD} AND NOT {PARENT}"
            ),
            params![provider.as_str(), gone.checkpoint],
        )?;
        Ok(changed > 0)
    }

    /// Clears the mark of claimed checkpoint `checkpoint`, still on the disk, so it is used
    /// again; when its layer was saved again meanwhile, it stays marked for a later delete.
    pub(crate) fn unclaim_checkpoint(
        &self,
        provider: &ProviderId,
        checkpoint: &str,
    ) -> Result<(), StoreError> {
        self.writer().execute(
            "UPDATE setup_checkpoints SET evicting = 0 \
             WHERE provider = ? AND checkpoint = ? AND evicting = 1 \
             AND NOT EXISTS (SELECT 1 FROM setup_checkpoints n WHERE n.provider = ? \
             AND n.layer_key = setup_checkpoints.layer_key AND n.evicting = 0)",
            params![provider.as_str(), checkpoint, provider.as_str()],
        )?;
        Ok(())
    }

    /// Forgets claimed checkpoint `checkpoint`, gone from the disk, and the Boxes that stood on
    /// its layer unless the layer was saved again; returns whether it was still tracked.
    pub(crate) fn forget_checkpoint(
        &self,
        provider: &ProviderId,
        checkpoint: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let key: Option<String> = tx
            .query_row(
                "DELETE FROM setup_checkpoints \
                 WHERE provider = ? AND checkpoint = ? AND evicting = 1 RETURNING layer_key",
                params![provider.as_str(), checkpoint],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(key) = &key {
            tx.execute(
                "DELETE FROM setup_checkpoint_pins WHERE provider = ?1 AND layer_key = ?2 \
                 AND NOT EXISTS (SELECT 1 FROM setup_checkpoints \
                 WHERE provider = ?1 AND layer_key = ?2)",
                params![provider.as_str(), key],
            )?;
        }
        tx.commit()?;
        drop(conn);
        Ok(key.is_some())
    }
}
