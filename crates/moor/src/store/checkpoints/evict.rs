//! Which setup checkpoints may go to free disk, in the order they go, and the Boxes that hold
//! some back.

use moor_core::domain::layers::LayerKey;
use moor_core::{BoxId, ProviderId};
use moor_schema::tables::{Boxes, SetupCheckpointPins as Pins, SetupCheckpoints as Checkpoints};
use sea_query::{DynIden, Expr, ExprTrait as _, IntoIden, OnConflict, Order, Query};

use crate::store::db::{self, DbExt as _, OptionalExtension};
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
fn held(c: &DynIden) -> Expr {
    let deepest = Query::select()
        .expr(Expr::col(("q", Pins::Layer)).max())
        .from_as(Pins::Table, "q")
        .and_where(Expr::col(("q", Pins::BoxId)).equals(("p", Pins::BoxId)))
        .and_where(Expr::col(("q", Pins::Provider)).equals(("p", Pins::Provider)))
        .take();
    let state = Expr::col((Boxes::Table, Boxes::State));
    let q = Query::select()
        .expr(Expr::val(1))
        .from_as(Pins::Table, "p")
        .inner_join(
            Boxes::Table,
            Expr::col((Boxes::Table, Boxes::Id)).equals(("p", Pins::BoxId)),
        )
        .and_where(Expr::col(("p", Pins::Provider)).equals((c.clone(), Checkpoints::Provider)))
        .and_where(Expr::col(("p", Pins::LayerKey)).equals((c.clone(), Checkpoints::LayerKey)))
        .and_where(
            state.clone().is_in(["pending", "setting_up"]).or(state
                .eq("setup_failed")
                .and(Expr::col(("p", Pins::Layer)).eq(db::sub(deepest)))),
        )
        .take();
    Expr::exists(q)
}

/// True for a checkpoint `c` that another checkpoint kept on the disk is a delta over, which
/// the provider refuses to delete first.
fn parent(c: &DynIden) -> Expr {
    let q = Query::select()
        .expr(Expr::val(1))
        .from_as(Checkpoints::Table, "d")
        .and_where(
            Expr::col(("d", Checkpoints::Provider)).equals((c.clone(), Checkpoints::Provider)),
        )
        .and_where(
            Expr::col(("d", Checkpoints::Parent)).equals((c.clone(), Checkpoints::Checkpoint)),
        )
        .take();
    Expr::exists(q)
}

/// True for a checkpoint `c` whose user began another chain of the same repo after `c`'s
/// chain began, as a Box at a newer commit does.
fn superseded(c: &DynIden) -> Expr {
    let q = Query::select()
        .expr(Expr::val(1))
        .from_as(Checkpoints::Table, "n")
        .and_where(
            Expr::col(("n", Checkpoints::Provider)).equals((c.clone(), Checkpoints::Provider)),
        )
        .and_where(Expr::col(("n", Checkpoints::User)).equals((c.clone(), Checkpoints::User)))
        .and_where(Expr::col(("n", Checkpoints::Repo)).equals((c.clone(), Checkpoints::Repo)))
        .and_where(Expr::col(("n", Checkpoints::Chain)).not_equals((c.clone(), Checkpoints::Chain)))
        .and_where(
            Expr::col(("n", Checkpoints::ChainAt)).gt(Expr::col((c.clone(), Checkpoints::ChainAt))),
        )
        .take();
    Expr::exists(q)
}

impl Store {
    /// Marks layer `layer`, keyed `key`, under `provider` as one Box `id` stands on.
    pub(crate) fn pin_checkpoint(
        &self,
        id: BoxId,
        provider: &ProviderId,
        key: &LayerKey,
        layer: usize,
    ) -> Result<(), StoreError> {
        self.writer().run(
            Query::insert()
                .into_table(Pins::Table)
                .columns([Pins::BoxId, Pins::Provider, Pins::LayerKey, Pins::Layer])
                .values_panic([
                    id.get().into(),
                    provider.as_str().into(),
                    key.as_str().into(),
                    i64::try_from(layer).unwrap_or(i64::MAX).into(),
                ])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
        Ok(())
    }

    /// Lets go of every checkpoint Box `id` stands on, once its disk no longer holds them.
    pub(crate) fn unpin_checkpoints(&self, id: BoxId) -> Result<(), StoreError> {
        self.writer().run(
            Query::delete()
                .from_table(Pins::Table)
                .and_where(Expr::col(Pins::BoxId).eq(id.get())),
        )?;
        Ok(())
    }

    /// The checkpoints under `provider` that may be claimed for deleting: no Box holds them back
    /// and none is a delta over them; superseded chains first, then the least recently used.
    pub(crate) fn evictable(&self, provider: &ProviderId) -> Result<Vec<Evictable>, StoreError> {
        let c = "c".into_iden();
        let filter = Expr::col((c.clone(), Checkpoints::Evicting))
            .eq(0)
            .and(held(&c).not());
        self.to_evict(provider, filter)
    }

    /// The checkpoints under `provider` claimed for deleting and still tracked, as a deleter cut
    /// short leaves them.
    pub(crate) fn evicting(&self, provider: &ProviderId) -> Result<Vec<Evictable>, StoreError> {
        let filter = Expr::col(("c", Checkpoints::Evicting)).ne(0);
        self.to_evict(provider, filter)
    }

    /// The leaf checkpoints under `provider` that `filter` keeps, in the order they go.
    fn to_evict(&self, provider: &ProviderId, filter: Expr) -> Result<Vec<Evictable>, StoreError> {
        let conn = self.reader()?;
        let c = "c".into_iden();
        let col = |col| (c.clone(), col);
        let q = Query::select()
            .columns([
                col(Checkpoints::LayerKey),
                col(Checkpoints::Checkpoint),
                col(Checkpoints::Repo),
                col(Checkpoints::Bytes),
            ])
            .expr(superseded(&c))
            .from_as(Checkpoints::Table, c.clone())
            .and_where(Expr::col(col(Checkpoints::Provider)).eq(provider.as_str()))
            .and_where(filter)
            .and_where(parent(&c).not())
            .order_by_expr(superseded(&c), Order::Desc)
            .order_by(col(Checkpoints::LastUsedAt), Order::Asc)
            .order_by(col(Checkpoints::CreatedAt), Order::Asc)
            .take();
        let rows = conn.rows(q, |r| {
            Ok::<_, StoreError>(Evictable {
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
        rows.collect()
    }

    /// Marks `gone` as being deleted, unless a Box began holding it back, a delta over it was
    /// saved, or it is marked already; returns whether it was marked, and so may be deleted.
    /// A marked checkpoint stays tracked, but no Box starts from it.
    pub(crate) fn claim_checkpoint(
        &self,
        provider: &ProviderId,
        gone: &Evictable,
    ) -> Result<bool, StoreError> {
        let c = Checkpoints::Table.into_iden();
        let changed = self.writer().run(
            Query::update()
                .table(Checkpoints::Table)
                .value(Checkpoints::Evicting, 1)
                .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                .and_where(Expr::col(Checkpoints::Checkpoint).eq(gone.checkpoint.as_str()))
                .and_where(Expr::col(Checkpoints::Evicting).eq(0))
                .and_where(held(&c).not())
                .and_where(parent(&c).not()),
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
        let saved_again = Query::select()
            .expr(Expr::val(1))
            .from_as(Checkpoints::Table, "n")
            .and_where(Expr::col(("n", Checkpoints::Provider)).eq(provider.as_str()))
            .and_where(
                Expr::col(("n", Checkpoints::LayerKey))
                    .equals((Checkpoints::Table, Checkpoints::LayerKey)),
            )
            .and_where(Expr::col(("n", Checkpoints::Evicting)).eq(0))
            .take();
        self.writer().run(
            Query::update()
                .table(Checkpoints::Table)
                .value(Checkpoints::Evicting, 0)
                .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                .and_where(Expr::col(Checkpoints::Checkpoint).eq(checkpoint))
                .and_where(Expr::col(Checkpoints::Evicting).eq(1))
                .and_where(Expr::exists(saved_again).not()),
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
            .row(
                Query::delete()
                    .from_table(Checkpoints::Table)
                    .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                    .and_where(Expr::col(Checkpoints::Checkpoint).eq(checkpoint))
                    .and_where(Expr::col(Checkpoints::Evicting).ne(0))
                    .returning_col(Checkpoints::LayerKey),
                |r| r.get(0),
            )
            .optional()?;
        if let Some(key) = &key {
            let tracked = Query::select()
                .expr(Expr::val(1))
                .from(Checkpoints::Table)
                .and_where(Expr::col(Checkpoints::Provider).eq(provider.as_str()))
                .and_where(Expr::col(Checkpoints::LayerKey).eq(key.as_str()))
                .take();
            tx.run(
                Query::delete()
                    .from_table(Pins::Table)
                    .and_where(Expr::col(Pins::Provider).eq(provider.as_str()))
                    .and_where(Expr::col(Pins::LayerKey).eq(key.as_str()))
                    .and_where(Expr::exists(tracked).not()),
            )?;
        }
        tx.commit()?;
        drop(conn);
        Ok(key.is_some())
    }
}
