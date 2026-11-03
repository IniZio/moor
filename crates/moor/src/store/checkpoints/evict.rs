//! Which setup checkpoints may go to free disk, in the order they go, and the Boxes that hold
//! some back.

use moor_core::domain::layers::LayerKey;
use moor_core::{BoxId, ProviderId};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};

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

/// A checkpoint no Box holds back.
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

    /// The checkpoints under `provider` no Box holds back, superseded chains first, then the
    /// least recently used.
    pub(crate) fn evictable(&self, provider: &ProviderId) -> Result<Vec<Evictable>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT c.layer_key, c.checkpoint, c.repo, c.bytes, {SUPERSEDED} \
             FROM setup_checkpoints c WHERE c.provider = ? AND NOT {HELD} \
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

    /// Forgets `gone`, unless a Box began holding it back or it was saved again meanwhile;
    /// returns what was forgotten, which [`Store::restore_checkpoint`] puts back if the delete
    /// fails, or `None` when it was not, and so may not be deleted.
    pub(crate) fn claim_checkpoint(
        &self,
        provider: &ProviderId,
        gone: &Evictable,
    ) -> Result<Option<Claimed>, StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let row = {
            let mut stmt = tx.prepare(&format!(
                "DELETE FROM setup_checkpoints AS c \
                 WHERE c.provider = ? AND c.layer_key = ? AND c.checkpoint = ? AND NOT {HELD} \
                 RETURNING *"
            ))?;
            let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
            let mut rows =
                stmt.query(params![provider.as_str(), gone.layer_key, gone.checkpoint])?;
            match rows.next()? {
                Some(r) => {
                    let values = (0..names.len())
                        .map(|i| r.get::<_, Value>(i))
                        .collect::<Result<Vec<_>, _>>()?;
                    Some(names.into_iter().zip(values).collect::<Vec<_>>())
                }
                None => None,
            }
        };
        let claimed = match row {
            Some(row) => {
                let mut stmt = tx.prepare(
                    "DELETE FROM setup_checkpoint_pins WHERE provider = ? AND layer_key = ? \
                     RETURNING box_id, layer",
                )?;
                let rows = stmt.query_map(params![provider.as_str(), gone.layer_key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
                Some(Claimed {
                    provider: provider.as_str().to_owned(),
                    layer_key: gone.layer_key.clone(),
                    row,
                    pins: rows.collect::<Result<_, _>>()?,
                })
            }
            None => None,
        };
        tx.commit()?;
        drop(conn);
        Ok(claimed)
    }

    /// Tracks a checkpoint claimed by [`Store::claim_checkpoint`] again, with the Boxes that
    /// stood on it, since it is still on the disk; nothing when its layer was saved again
    /// meanwhile.
    pub(crate) fn restore_checkpoint(&self, claimed: &Claimed) -> Result<(), StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let cols: Vec<&str> = claimed.row.iter().map(|(n, _)| n.as_str()).collect();
        let marks = vec!["?"; cols.len()].join(", ");
        let restored = tx.execute(
            &format!(
                "INSERT OR IGNORE INTO setup_checkpoints ({}) VALUES ({marks})",
                cols.join(", ")
            ),
            params_from_iter(claimed.row.iter().map(|(_, v)| v)),
        )?;
        if restored > 0 {
            for (box_id, layer) in &claimed.pins {
                tx.execute(
                    "INSERT OR IGNORE INTO setup_checkpoint_pins \
                     (box_id, provider, layer_key, layer) VALUES (?, ?, ?, ?)",
                    params![box_id, claimed.provider, claimed.layer_key, layer],
                )?;
            }
        }
        tx.commit()?;
        drop(conn);
        Ok(())
    }
}

/// A checkpoint's row and pins as [`Store::claim_checkpoint`] took them.
#[derive(Debug)]
pub(crate) struct Claimed {
    /// The provider it is under.
    provider: String,
    /// Its layer's key.
    layer_key: String,
    /// Its row, column by column.
    row: Vec<(String, Value)>,
    /// The Boxes that stood on it and at which layer.
    pins: Vec<(i64, i64)>,
}
