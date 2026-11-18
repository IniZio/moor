//! The runs of one workflow in a list, newest first, as the portal's runs list reads them.

use moor_core::workflow::{WorkflowId, WorkflowRun};
use moor_schema::tables::WorkflowRuns;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::row::{read_run, run_of, RUN_COLS};
use crate::store::db::DbExt as _;
use crate::store::{Store, StoreError};

impl Store {
    /// The latest `limit` runs of `workflow` of any repo, newest due time first, or only the run
    /// with id `only` when one is given.
    pub(crate) fn workflow_run_list(
        &self,
        workflow: WorkflowId,
        only: Option<i64>,
        limit: u64,
    ) -> Result<Vec<WorkflowRun>, StoreError> {
        let mut select = Query::select();
        select
            .columns(RUN_COLS)
            .from(WorkflowRuns::Table)
            .and_where(Expr::col(WorkflowRuns::Workflow).eq(workflow.get()));
        if let Some(id) = only {
            select.and_where(Expr::col(WorkflowRuns::Id).eq(id));
        }
        select
            .order_by(WorkflowRuns::DueAt, Order::Desc)
            .order_by(WorkflowRuns::Id, Order::Desc)
            .limit(limit);
        self.reader()?
            .rows(&mut select, |r| read_run(r, 0))?
            .map(|row| run_of(workflow, &row?))
            .collect()
    }
}
