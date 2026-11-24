//! What Boxes are tied to, read for many Boxes at once: the workflow run that made each, the node
//! it runs on and the place that started it. One read per tie, however many Boxes are asked about.

use std::collections::HashMap;

use moor_core::view::ties::WorkflowRef;
use moor_core::view::BoxTies;
use moor_core::BoxId;
use moor_schema::tables::{BoxState, Boxes, WorkflowRuns, Workflows};
use sea_query::{Expr, ExprTrait as _, JoinType, Order, Query};

use super::db::{Db, DbExt as _};
use super::{Store, StoreError};

impl Store {
    /// The ties of each Box in `ids` that has any. A Box's workflow is the one of its latest run,
    /// the highest run id, so a Box a later run reused names that run's workflow. A workflow that was
    /// deleted took its runs with it, so its Boxes name none. A Box's machine is the node it runs on,
    /// none while it is on none. A Box's task is the place whose agent started it.
    pub(crate) fn ties_of(&self, ids: &[BoxId]) -> Result<HashMap<BoxId, BoxTies>, StoreError> {
        let mut ties: HashMap<BoxId, BoxTies> = HashMap::new();
        if ids.is_empty() {
            return Ok(ties);
        }
        let reader = self.reader()?;
        let wanted = || ids.iter().map(|id| id.get());
        let runs = reader.rows(
            Query::select()
                .column((WorkflowRuns::Table, WorkflowRuns::Box))
                .column((Workflows::Table, Workflows::Name))
                .column((WorkflowRuns::Table, WorkflowRuns::Repo))
                .column((WorkflowRuns::Table, WorkflowRuns::PullNumber))
                .from(WorkflowRuns::Table)
                .inner_join(
                    Workflows::Table,
                    Expr::col((Workflows::Table, Workflows::Id))
                        .equals((WorkflowRuns::Table, WorkflowRuns::Workflow)),
                )
                .and_where(Expr::col((WorkflowRuns::Table, WorkflowRuns::Box)).is_in(wanted()))
                .order_by((WorkflowRuns::Table, WorkflowRuns::Id), Order::Asc),
            |r| {
                let (box_id, name, repo, number): (i64, String, String, Option<i64>) =
                    (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
                Ok::<_, super::db::Error>((box_id, name, number.map(|n| format!("{repo}#{n}"))))
            },
        )?;
        for run in runs {
            let (box_id, name, pull) = run?;
            ties.entry(BoxId::new(box_id)).or_default().workflow = Some(WorkflowRef { name, pull });
        }
        let nodes = reader.rows(
            Query::select()
                .columns([BoxState::Box, BoxState::Node])
                .from(BoxState::Table)
                .and_where(Expr::col(BoxState::Box).is_in(wanted()))
                .and_where(Expr::col(BoxState::Node).is_not_null()),
            |r| Ok::<_, super::db::Error>((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        for node in nodes {
            let (box_id, machine) = node?;
            ties.entry(BoxId::new(box_id)).or_default().machine = Some(machine);
        }
        for (box_id, place) in task_names(&reader, ids)? {
            ties.entry(BoxId::new(box_id)).or_default().task = Some(place);
        }
        Ok(ties)
    }
}

/// The name of the place that started each Box in `ids` that a place started, by the Box's id.
fn task_names(conn: &dyn Db, ids: &[BoxId]) -> Result<Vec<(i64, String)>, StoreError> {
    let started = Query::select()
        .column(("w", Boxes::Id))
        .column(("p", Boxes::Name))
        .from_as(Boxes::Table, "w")
        .join_as(
            JoinType::InnerJoin,
            Boxes::Table,
            "p",
            Expr::col(("p", Boxes::Id)).equals(("w", Boxes::Task)),
        )
        .and_where(Expr::col(("w", Boxes::Id)).is_in(ids.iter().map(|id| id.get())))
        .take();
    conn.rows(started, |r| {
        Ok::<_, super::db::Error>((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?
    .collect::<Result<_, _>>()
    .map_err(Into::into)
}
