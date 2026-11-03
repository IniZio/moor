//! Setup checkpoints: the provider checkpoint that holds each setup layer, by layer key.

use moor_core::domain::layers::LayerKey;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Checkpoint;
use moor_core::user::UserId;
use moor_core::{BoxPath, ProviderId};
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
    /// Where a new Box may start from it; none until [`Store::place_checkpoint`] records it.
    pub(crate) place: Option<Place>,
}

/// What a new Box started from a checkpoint inherits: the saved disk's workdir, and what the
/// layers below it took to set up from scratch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    /// The workdir on the checkpointed disk.
    pub(crate) workdir: BoxPath,
    /// Milliseconds the clone and the lines up to this layer took on a Box set up from scratch.
    pub(crate) setup_ms: u64,
}

/// Whose setup a checkpoint holds: a user's chain of layers over one repo at one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lineage {
    /// The user whose Box saved it.
    pub(crate) user: UserId,
    /// The repo, `OWNER/NAME`.
    pub(crate) repo: String,
    /// The clone layer's key, shared by every layer of the chain.
    pub(crate) chain: LayerKey,
}

impl Store {
    /// Records `made` as the checkpoint of layer `key` of `lineage` under `provider`, replacing
    /// an earlier one and its place, unless that one is being deleted.
    pub(crate) fn record_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
        lineage: &Lineage,
        made: &Checkpoint,
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        let bytes = made.bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX));
        let chain = lineage.chain.as_str();
        self.writer().execute(
            "INSERT INTO setup_checkpoints (provider, layer_key, checkpoint, repo, bytes, \
             created_at, last_used_at, user, chain, chain_at, parent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, COALESCE((SELECT MIN(chain_at) \
             FROM setup_checkpoints WHERE provider = ?1 AND chain = ?8), ?6), ?9) \
             ON CONFLICT (provider, layer_key) WHERE evicting = 0 DO UPDATE SET \
             checkpoint = excluded.checkpoint, repo = excluded.repo, bytes = excluded.bytes, \
             created_at = excluded.created_at, last_used_at = excluded.last_used_at, \
             user = excluded.user, chain = excluded.chain, chain_at = excluded.chain_at, \
             parent = excluded.parent, workdir = NULL, setup_ms = NULL",
            params![
                provider.as_str(),
                key.as_str(),
                made.id,
                lineage.repo,
                bytes,
                now,
                lineage.user.get(),
                chain,
                made.parent
            ],
        )?;
        Ok(())
    }

    /// The checkpoint of layer `key` under `provider`, if one is recorded and not being deleted.
    pub(crate) fn checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
    ) -> Result<Option<CheckpointRow>, StoreError> {
        let row = self
            .reader()?
            .query_row(
                "SELECT checkpoint, repo, bytes, created_at, last_used_at, workdir, setup_ms \
                 FROM setup_checkpoints \
                 WHERE provider = ? AND layer_key = ? AND evicting = 0",
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
                        place: place(r.get(5)?, r.get(6)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Records where a new Box may start from the checkpoint of layer `key` under `provider`.
    pub(crate) fn place_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
        place: &Place,
    ) -> Result<(), StoreError> {
        let ms = i64::try_from(place.setup_ms).unwrap_or(i64::MAX);
        self.writer().execute(
            "UPDATE setup_checkpoints SET workdir = ?, setup_ms = ? \
             WHERE provider = ? AND layer_key = ? AND evicting = 0",
            params![place.workdir.as_str(), ms, provider.as_str(), key.as_str()],
        )?;
        Ok(())
    }

    /// Marks the checkpoint of layer `key` under `provider` used now; returns whether it exists.
    pub(crate) fn use_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
    ) -> Result<bool, StoreError> {
        let changed = self.writer().execute(
            "UPDATE setup_checkpoints SET last_used_at = ? \
             WHERE provider = ? AND layer_key = ? AND evicting = 0",
            params![self.now().as_millis(), provider.as_str(), key.as_str()],
        )?;
        Ok(changed > 0)
    }
}

/// The place stored as `workdir` and `setup_ms`; none when either is missing or unreadable.
fn place(workdir: Option<String>, setup_ms: Option<i64>) -> Option<Place> {
    Some(Place {
        workdir: BoxPath::restore(&workdir?).ok()?,
        setup_ms: u64::try_from(setup_ms?).ok()?,
    })
}

mod evict;

#[cfg(test)]
mod tests;
