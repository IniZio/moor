//! The node a Box asked for or holds, and the labels it requires of one.

use std::collections::HashSet;

use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::name::NodeName;
use moor_core::BoxId;
use moor_schema::tables::Boxes;
use sea_query::{Asterisk, Expr, ExprTrait as _, Order, Query};

use super::{Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension};

/// Boxes holding a slot on `node` as seen on `conn`, counted the way admission counts them,
/// and Boxes setting up there.
pub(super) fn live_on_node_in(
    conn: &dyn Db,
    node: &NodeName,
) -> Result<(usize, usize), StoreError> {
    let rows = conn.rows(
        Query::select()
            .columns([Boxes::DriverState, Boxes::State])
            .expr(Expr::col(Asterisk).count())
            .from(Boxes::Table)
            .and_where(Expr::col(Boxes::Node).eq(node.as_str()))
            .group_by_columns([Boxes::DriverState, Boxes::State]),
        |r| {
            Ok::<_, StoreError>((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    )?;
    let (mut used, mut starting) = (0, 0);
    for row in rows {
        let (driver, state, n) = row?;
        let n = usize::try_from(n).unwrap_or(0);
        if driver.and_then(|d| BoxState::parse(&d)).is_some_and(counts) {
            used += n;
        }
        if state == BoxLifecycle::SettingUp.as_str() {
            starting += n;
        }
    }
    Ok((used, starting))
}

impl Store {
    /// The Boxes whose lifecycle is Ready on `node`.
    pub(crate) fn ready_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .column(Boxes::Id)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Node).eq(node.as_str()))
                .and_where(Expr::col(Boxes::State).eq(BoxLifecycle::Ready.as_str())),
            |r| Ok(BoxId::new(r.get(0)?)),
        )?;
        Ok(rows.collect::<Result<_, crate::store::db::Error>>()?)
    }

    /// One Box per owner among those on `node` whose disk is still there, Ready or asleep. An
    /// awake Box is preferred, since its owner's thread or agent is the one in use.
    pub(crate) fn one_live_per_owner_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .columns([Boxes::Id, Boxes::User])
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Node).eq(node.as_str()))
                .and_where(
                    Expr::col(Boxes::State)
                        .is_in([BoxLifecycle::Ready.as_str(), BoxLifecycle::Asleep.as_str()]),
                )
                .order_by_expr(
                    Expr::col(Boxes::State).eq(BoxLifecycle::Ready.as_str()),
                    Order::Desc,
                )
                .order_by(Boxes::Id, Order::Asc),
            |r| Ok::<_, StoreError>((BoxId::new(r.get(0)?), r.get::<_, Option<i64>>(1)?)),
        )?;
        let mut owners = HashSet::new();
        let mut picked = Vec::new();
        for row in rows {
            let (id, owner) = row?;
            if owners.insert(owner) {
                picked.push(id);
            }
        }
        Ok(picked)
    }

    /// What Box `id` asks of a node: the node it named or holds, and its labels.
    pub(crate) fn box_spec(&self, id: BoxId) -> Result<BoxSpec, StoreError> {
        let conn = self.reader()?;
        let found: Option<(Option<String>, String)> = conn
            .row(
                Query::select()
                    .columns([Boxes::Node, Boxes::NodeLabels])
                    .from(Boxes::Table)
                    .and_where(Expr::col(Boxes::Id).eq(id.get())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (node, labels) = found.unwrap_or_else(|| (None, "[]".to_owned()));
        let labels = serde_json::from_str(&labels)
            .map_err(|e| StoreError::Corrupt(format!("box {} node labels: {e}", id.get())))?;
        Ok(BoxSpec {
            labels,
            node: node.and_then(|n| NodeName::parse(&n).ok()),
        })
    }

    /// Records what Box `id` asks of a node.
    pub(crate) fn set_box_spec(&self, id: BoxId, spec: &BoxSpec) -> Result<(), StoreError> {
        let labels = serde_json::to_string(&spec.labels)
            .map_err(|e| StoreError::Corrupt(format!("node labels: {e}")))?;
        self.writer().run(
            Query::update()
                .table(Boxes::Table)
                .value(Boxes::Node, spec.node.as_ref().map(NodeName::as_str))
                .value(Boxes::NodeLabels, labels)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
        )?;
        Ok(())
    }

    /// Puts Box `id` on `node` when it is on none; returns whether it was.
    pub(crate) fn place_unplaced(&self, id: BoxId, node: &NodeName) -> Result<bool, StoreError> {
        let n = self.writer().run(
            Query::update()
                .table(Boxes::Table)
                .value(Boxes::Node, node.as_str())
                .and_where(Expr::col(Boxes::Id).eq(id.get()))
                .and_where(Expr::col(Boxes::Node).is_null()),
        )?;
        Ok(n > 0)
    }

    /// Pending Boxes waiting for `node`: those suspended as a last resort first, then oldest first.
    pub(crate) fn pending_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            Query::select()
                .column(Boxes::Id)
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Node).eq(node.as_str()))
                .and_where(Expr::col(Boxes::State).eq(BoxLifecycle::Pending.as_str()))
                .order_by_expr(Expr::col(Boxes::ResumeFirstAt).is_null(), Order::Asc)
                .order_by(Boxes::Id, Order::Asc),
            |r| r.get::<_, i64>(0),
        )?;
        rows.map(|r| Ok(BoxId::new(r?))).collect()
    }
}
