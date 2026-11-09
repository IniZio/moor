//! The node a Box asked for or holds, and the labels it requires of one.

use std::collections::HashSet;

use moor_core::decide::capacity::counts;
use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::hands::HandsState;
use moor_core::name::NodeName;
use moor_core::BoxId;
use moor_schema::tables::{BoxHands, BoxState, Boxes};
use sea_query::{Asterisk, Expr, ExprTrait as _, Order, Query};

use super::hands::join_current;
use super::select::{join_state, state_update};
use super::{Store, StoreError};
use crate::store::db::{Db, DbExt as _, OptionalExtension};

/// A column of `boxes`, where a read joins `box_state` or `box_hands`.
const fn b(c: Boxes) -> (Boxes, Boxes) {
    (Boxes::Table, c)
}

/// A column of `box_state`.
const fn st(c: BoxState) -> (BoxState, BoxState) {
    (BoxState::Table, c)
}

/// A column of `box_hands`.
const fn h(c: BoxHands) -> (BoxHands, BoxHands) {
    (BoxHands::Table, c)
}

/// Boxes holding a slot on `node` as seen on `conn`, counted the way admission counts them,
/// and Boxes setting up there.
pub(super) fn live_on_node_in(
    conn: &dyn Db,
    node: &NodeName,
) -> Result<(usize, usize), StoreError> {
    let rows = conn.rows(
        join_current(join_state(
            Query::select()
                .column(h(BoxHands::State))
                .column(st(BoxState::State))
                .expr(Expr::col(Asterisk).count())
                .from(Boxes::Table),
        ))
        .and_where(Expr::col(st(BoxState::Node)).eq(node.as_str()))
        .group_by_col(h(BoxHands::State))
        .group_by_col(st(BoxState::State)),
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
        if driver
            .and_then(|d| HandsState::parse(&d))
            .is_some_and(counts)
        {
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
                .column(BoxState::Box)
                .from(BoxState::Table)
                .and_where(Expr::col(BoxState::Node).eq(node.as_str()))
                .and_where(Expr::col(BoxState::State).eq(BoxLifecycle::Ready.as_str())),
            |r| Ok(BoxId::new(r.get(0)?)),
        )?;
        Ok(rows.collect::<Result<_, crate::store::db::Error>>()?)
    }

    /// One Box per owner among those on `node` whose disk is still there, Ready or asleep. An
    /// awake Box is preferred, since its owner's thread or agent is the one in use.
    pub(crate) fn one_live_per_owner_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            join_state(
                Query::select()
                    .columns([b(Boxes::Id), b(Boxes::User)])
                    .from(Boxes::Table),
            )
            .and_where(Expr::col(st(BoxState::Node)).eq(node.as_str()))
            .and_where(
                Expr::col(st(BoxState::State))
                    .is_in([BoxLifecycle::Ready.as_str(), BoxLifecycle::Asleep.as_str()]),
            )
            .order_by_expr(
                Expr::col(st(BoxState::State)).eq(BoxLifecycle::Ready.as_str()),
                Order::Desc,
            )
            .order_by(b(Boxes::Id), Order::Asc),
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
                join_state(
                    Query::select()
                        .column(st(BoxState::Node))
                        .column(b(Boxes::NodeLabels))
                        .from(Boxes::Table),
                )
                .and_where(Expr::col(b(Boxes::Id)).eq(id.get())),
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

    /// Records the node Box `id` is placed on, or none.
    pub(crate) fn set_box_node(
        &self,
        id: BoxId,
        node: Option<&NodeName>,
    ) -> Result<(), StoreError> {
        self.writer()
            .run(state_update(id).value(BoxState::Node, node.map(NodeName::as_str)))?;
        Ok(())
    }

    /// Puts Box `id` on `node` when it is on none; returns whether it was.
    pub(crate) fn place_unplaced(&self, id: BoxId, node: &NodeName) -> Result<bool, StoreError> {
        let n = self.writer().run(
            state_update(id)
                .value(BoxState::Node, node.as_str())
                .and_where(Expr::col(BoxState::Node).is_null()),
        )?;
        Ok(n > 0)
    }

    /// The queue for `node`: Boxes suspended as a last resort, then other resumes, then new
    /// starts, each oldest first.
    pub(crate) fn pending_on(&self, node: &NodeName) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            join_current(join_state(
                Query::select().column(b(Boxes::Id)).from(Boxes::Table),
            ))
            .and_where(Expr::col(st(BoxState::Node)).eq(node.as_str()))
            .and_where(Expr::col(st(BoxState::State)).eq(BoxLifecycle::Pending.as_str()))
            .order_by_expr(Expr::col(st(BoxState::ResumeFirstAt)).is_null(), Order::Asc)
            .order_by_expr(
                Expr::case(
                    Expr::col(h(BoxHands::State)).eq(HandsState::Asleep.as_str()),
                    0,
                )
                .finally(1)
                .into(),
                Order::Asc,
            )
            .order_by(b(Boxes::Id), Order::Asc),
            |r| r.get::<_, i64>(0),
        )?;
        rows.map(|r| Ok(BoxId::new(r?))).collect()
    }
}
