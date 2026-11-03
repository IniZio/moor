//! Setup checkpoints: the provider checkpoint that holds each setup layer, by layer key.

use moor_core::domain::layers::LayerKey;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Checkpoint;
use moor_core::ProviderId;
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

/// One setup layer saved under a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckpointRow {
    /// The provider's checkpoint id.
    pub(crate) checkpoint: String,
    /// The repo whose layer it holds, `OWNER/NAME`.
    pub(crate) repo: String,
    /// Its size on the host, when reported.
    pub(crate) bytes: Option<u64>,
    /// When it was saved.
    pub(crate) created_at: Ts,
    /// When a Box last started from it, or when it was saved.
    pub(crate) last_used_at: Ts,
}

impl Store {
    /// Records `made` as the checkpoint of layer `key` of `repo` under `provider`, replacing
    /// an earlier one.
    pub(crate) fn record_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
        repo: &str,
        made: &Checkpoint,
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        let bytes = made.bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX));
        self.writer().execute(
            "INSERT INTO setup_checkpoints \
             (provider, layer_key, checkpoint, repo, bytes, created_at, last_used_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (provider, layer_key) DO UPDATE SET checkpoint = excluded.checkpoint, \
             repo = excluded.repo, bytes = excluded.bytes, created_at = excluded.created_at, \
             last_used_at = excluded.last_used_at",
            params![
                provider.as_str(),
                key.as_str(),
                made.id,
                repo,
                bytes,
                now,
                now
            ],
        )?;
        Ok(())
    }

    /// The checkpoint of layer `key` under `provider`, if one is recorded.
    pub(crate) fn checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
    ) -> Result<Option<CheckpointRow>, StoreError> {
        let row = self
            .reader()?
            .query_row(
                "SELECT checkpoint, repo, bytes, created_at, last_used_at FROM setup_checkpoints \
                 WHERE provider = ? AND layer_key = ?",
                params![provider.as_str(), key.as_str()],
                |r| {
                    Ok(CheckpointRow {
                        checkpoint: r.get(0)?,
                        repo: r.get(1)?,
                        bytes: r
                            .get::<_, Option<i64>>(2)?
                            .and_then(|b| u64::try_from(b).ok()),
                        created_at: Ts::from_millis(r.get(3)?),
                        last_used_at: Ts::from_millis(r.get(4)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Marks the checkpoint of layer `key` under `provider` used now; returns whether it exists.
    pub(crate) fn use_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
    ) -> Result<bool, StoreError> {
        let changed = self.writer().execute(
            "UPDATE setup_checkpoints SET last_used_at = ? WHERE provider = ? AND layer_key = ?",
            params![self.now().as_millis(), provider.as_str(), key.as_str()],
        )?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests;
