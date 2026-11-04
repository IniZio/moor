//! Setup checkpoints: the provider checkpoint that holds each setup layer, by layer key.

use moor_core::domain::layers::LayerKey;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Checkpoint;
use moor_core::user::UserId;
use moor_core::{BoxPath, ProviderId};
use moor_schema::tables::SetupCheckpoints as Checkpoints;
use sea_query::{Expr, ExprTrait as _, Query, Value};

use crate::store::db::{DbExt as _, OptionalExtension, TransactionBehavior};
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
        let provider = provider.as_str();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let chain_began = Query::select()
            .expr(Expr::col(Checkpoints::ChainAt).min())
            .from(Checkpoints::Table)
            .and_where(Expr::col(Checkpoints::Provider).eq(provider))
            .and_where(Expr::col(Checkpoints::Chain).eq(chain))
            .take();
        let chain_at = tx
            .row(chain_began, |r| r.get::<_, Option<i64>>(0))?
            .unwrap_or(now);
        let layer: [(Checkpoints, Value); 9] = [
            (Checkpoints::Checkpoint, made.id.clone().into()),
            (Checkpoints::Repo, lineage.repo.clone().into()),
            (Checkpoints::Bytes, bytes.into()),
            (Checkpoints::CreatedAt, now.into()),
            (Checkpoints::LastUsedAt, now.into()),
            (Checkpoints::User, lineage.user.get().into()),
            (Checkpoints::Chain, chain.into()),
            (Checkpoints::ChainAt, chain_at.into()),
            (Checkpoints::Parent, made.parent.clone().into()),
        ];
        // The unique index covers only checkpoints not being deleted, and a partial index
        // cannot be an upsert's target once its predicate is a bound value, so replace or add.
        let replaced = tx.run(
            Query::update()
                .table(Checkpoints::Table)
                .values(layer.iter().map(|(c, v)| (*c, Expr::val(v.clone()))))
                .value(Checkpoints::Workdir, Expr::val(None::<String>))
                .value(Checkpoints::SetupMs, Expr::val(None::<i64>))
                .and_where(Expr::col(Checkpoints::Provider).eq(provider))
                .and_where(Expr::col(Checkpoints::LayerKey).eq(key.as_str()))
                .and_where(Expr::col(Checkpoints::Evicting).eq(0)),
        )?;
        if replaced == 0 {
            tx.run(
                Query::insert()
                    .into_table(Checkpoints::Table)
                    .columns(
                        [Checkpoints::Provider, Checkpoints::LayerKey]
                            .into_iter()
                            .chain(layer.iter().map(|(c, _)| *c)),
                    )
                    .values_panic(
                        [provider.into(), key.as_str().into()]
                            .into_iter()
                            .chain(layer.into_iter().map(|(_, v)| Expr::val(v))),
                    ),
            )?;
        }
        tx.commit()?;
        drop(conn);
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
            .row(
                Query::select()
                    .columns([
                        Checkpoints::Checkpoint,
                        Checkpoints::Repo,
                        Checkpoints::Bytes,
                        Checkpoints::CreatedAt,
                        Checkpoints::LastUsedAt,
                        Checkpoints::Workdir,
                        Checkpoints::SetupMs,
                    ])
                    .from(Checkpoints::Table)
                    .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                    .and_where(Expr::col(Checkpoints::LayerKey).eq(key.as_str()))
                    .and_where(Expr::col(Checkpoints::Evicting).eq(0)),
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
        self.writer().run(
            Query::update()
                .table(Checkpoints::Table)
                .value(Checkpoints::Workdir, place.workdir.as_str())
                .value(Checkpoints::SetupMs, ms)
                .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                .and_where(Expr::col(Checkpoints::LayerKey).eq(key.as_str()))
                .and_where(Expr::col(Checkpoints::Evicting).eq(0)),
        )?;
        Ok(())
    }

    /// Marks the checkpoint of layer `key` under `provider` used now; returns whether it exists.
    pub(crate) fn use_checkpoint(
        &self,
        provider: &ProviderId,
        key: &LayerKey,
    ) -> Result<bool, StoreError> {
        let changed = self.writer().run(
            Query::update()
                .table(Checkpoints::Table)
                .value(Checkpoints::LastUsedAt, self.now().as_millis())
                .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                .and_where(Expr::col(Checkpoints::LayerKey).eq(key.as_str()))
                .and_where(Expr::col(Checkpoints::Evicting).eq(0)),
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
