//! The work Boxes a place's agent started, with the two times a task's Box line needs.

use std::collections::HashMap;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};
use moor_schema::tables::{Boxes, Events};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::db::DbExt as _;
use super::reads::query_boxes;
use super::{BoxRow, Store, StoreError};

/// A work Box of a place, with the first time its provisioning ended and the time it closed.
pub(crate) struct TaskBox {
    /// The Box.
    pub(crate) row: BoxRow,
    /// When the Box first reached `working`, which is when provisioning ended; none before.
    pub(crate) ready_at: Option<Ts>,
    /// When the Box recorded `closed`; none while it is open.
    pub(crate) closed_at: Option<Ts>,
}

/// The branch a task's work Boxes share, and the one of them still open on it.
pub(crate) struct TaskBranch {
    /// The branch of the first Box the place started.
    pub(crate) branch: String,
    /// The task's Box that is open on that branch, if one is.
    pub(crate) open: Option<BoxName>,
}

impl Store {
    /// The branch of the first Box place `place` started, which every later Box of the task
    /// starts on; none before the place started one.
    pub(crate) fn task_branch(&self, place: BoxId) -> Result<Option<TaskBranch>, StoreError> {
        let tied = Expr::col((Boxes::Table, Boxes::Task)).eq(place.get());
        let rows = query_boxes(&self.reader()?, tied, Order::Asc, None)?;
        let Some(branch) = rows.iter().find_map(|r| r.branch.clone()) else {
            return Ok(None);
        };
        let open = rows
            .iter()
            .find(|r| {
                r.branch.as_ref() == Some(&branch)
                    && !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
            })
            .map(|r| r.name.clone());
        Ok(Some(TaskBranch { branch, open }))
    }

    /// The Boxes whose agent-started tie is place `place`, open or closed, oldest first. Each takes
    /// the time of its first `working` event, since a woken Box logs `working` again, and of its
    /// `closed` event. One read per kind of fact, however many Boxes the place started.
    pub(crate) fn task_boxes(&self, place: BoxId) -> Result<Vec<TaskBox>, StoreError> {
        let tied = Expr::col((Boxes::Table, Boxes::Task)).eq(place.get());
        let reader = self.reader()?;
        let rows = query_boxes(&reader, tied, Order::Asc, None)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let times = reader.rows(
            Query::select()
                .column(Events::Box)
                .column(Events::Kind)
                .expr(Expr::col(Events::Ts).min())
                .from(Events::Table)
                .and_where(Expr::col(Events::Box).is_in(rows.iter().map(|r| r.id.get())))
                .and_where(Expr::col(Events::Kind).is_in(["working", "closed"]))
                .group_by_columns([Events::Box, Events::Kind]),
            |r| {
                Ok::<_, super::db::Error>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )?;
        let mut first: HashMap<(i64, String), Ts> = HashMap::new();
        for time in times {
            let (id, kind, at) = time?;
            first.insert((id, kind), Ts::from_millis(at));
        }
        Ok(rows
            .into_iter()
            .map(|row| {
                let at = |kind: &str| first.get(&(row.id.get(), kind.to_owned())).copied();
                TaskBox {
                    ready_at: at("working"),
                    closed_at: at("closed"),
                    row,
                }
            })
            .collect())
    }
}
