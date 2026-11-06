//! Which runner holds each machine's link: recorded from each runner's reports, and read to
//! route a machine's ops to that runner.

use std::time::Duration;

use moor_core::request::{MachineLink, RunnerAddr};
use moor_core::seam::clock::Ts;
use moor_schema::tables::MachineLinks;
use sea_query::{Expr, ExprTrait as _, Order, Query};

use super::{is_enrolled, link_insert, link_row, link_select};
use crate::store::db::{self, Db, DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// How long a link stays routable after its runner last reported; a runner renews well within.
pub(crate) const LINK_STALE: Duration = Duration::from_secs(60);

impl Store {
    /// Records runner `runner`'s whole view of its links at `now`, `draining` when it is shutting
    /// down: each machine it lists that is enrolled and not held by a newer live link elsewhere,
    /// and drops the machines it held but no longer lists; what the store holds afterwards, by
    /// machine id.
    pub(crate) fn set_machine_links(
        &self,
        runner: &RunnerAddr,
        draining: bool,
        links: &[MachineLink],
        now: Ts,
    ) -> Result<Vec<MachineLink>, StoreError> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        for link in links {
            put_link(&tx, runner, link, now)?;
        }
        let listed: Vec<i64> = links.iter().map(|l| l.machine_id).collect();
        tx.run(
            Query::delete()
                .from_table(MachineLinks::Table)
                .cond_where(
                    Expr::col(MachineLinks::RunnerAddr)
                        .eq(runner.as_str())
                        .or(Expr::col(MachineLinks::RunnerAddr).is_null()),
                )
                .and_where(Expr::col(MachineLinks::MachineId).is_not_in(listed)),
        )?;
        let draining_since = if draining {
            Expr::col(MachineLinks::DrainingSince).if_null(now.as_millis())
        } else {
            Expr::val(Option::<i64>::None)
        };
        tx.run(
            Query::update()
                .table(MachineLinks::Table)
                .value(MachineLinks::HeardAt, now.as_millis())
                .value(MachineLinks::DrainingSince, draining_since)
                .and_where(Expr::col(MachineLinks::RunnerAddr).eq(runner.as_str())),
        )?;
        let held = {
            let rows = tx.rows(
                link_select().order_by((MachineLinks::Table, MachineLinks::MachineId), Order::Asc),
                link_row,
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        drop(conn);
        Ok(held)
    }

    /// The runner that holds machine `machine_id`'s link at `now`; none when no runner holds
    /// a link to it that is up, not draining, and reported within [`LINK_STALE`].
    pub(crate) fn machine_runner(
        &self,
        machine_id: i64,
        now: Ts,
    ) -> Result<Option<RunnerAddr>, StoreError> {
        let conn = self.reader()?;
        let state = conn
            .row(
                link_select().and_where(
                    Expr::col((MachineLinks::Table, MachineLinks::MachineId)).eq(machine_id),
                ),
                state_of,
            )
            .optional()?;
        Ok(state.and_then(|s| s.live(now).cloned()))
    }
}

/// Records `link` as `runner` holds it, unless a newer live link on another runner holds its
/// machine or the machine is not enrolled.
fn put_link(
    tx: &dyn Db,
    runner: &RunnerAddr,
    link: &MachineLink,
    now: Ts,
) -> Result<(), StoreError> {
    let stored = tx
        .row(
            link_select().and_where(
                Expr::col((MachineLinks::Table, MachineLinks::MachineId)).eq(link.machine_id),
            ),
            |r| Ok((link_row(r)?, state_of(r)?)),
        )
        .optional()?;
    if let Some((held, state)) = &stored {
        let elsewhere = state.live(now).is_some_and(|other| other != runner);
        if elsewhere && (!link.online || held.since >= link.since) {
            return Ok(());
        }
    }
    tx.run(
        Query::delete()
            .from_table(MachineLinks::Table)
            .and_where(Expr::col(MachineLinks::MachineId).eq(link.machine_id)),
    )?;
    if !is_enrolled(tx, link.machine_id)? {
        return Ok(());
    }
    // A relinked agent has not reported yet, but its node still reports memory.
    let memory = link.memory.or_else(|| stored.and_then(|(l, _)| l.memory));
    tx.run(link_insert(link, memory, runner)?)?;
    Ok(())
}

/// Where one machine's stored link stands.
enum LinkState {
    /// Up on `runner`, which last reported it at `heard_at`.
    Linked {
        /// The runner holding it.
        runner: RunnerAddr,
        /// When that runner last reported.
        heard_at: Ts,
    },
    /// Up on a runner that is shutting down.
    Draining,
    /// Down, or not reported by any runner that names itself.
    Offline,
}

impl LinkState {
    /// The runner to route to at `now`: a link up, not draining, and heard within [`LINK_STALE`].
    fn live(&self, now: Ts) -> Option<&RunnerAddr> {
        let fresh = |heard_at: &Ts| {
            let stale = i64::try_from(LINK_STALE.as_millis()).unwrap_or(i64::MAX);
            now.as_millis().saturating_sub(heard_at.as_millis()) <= stale
        };
        match self {
            Self::Linked { runner, heard_at } if fresh(heard_at) => Some(runner),
            _ => None,
        }
    }
}

/// The link state of a [`link_select`] row.
fn state_of(r: &db::Row<'_>) -> db::Result<LinkState> {
    let online: bool = r.get(1)?;
    let (runner, heard_at, draining): (Option<String>, Option<i64>, Option<i64>) =
        (r.get(18)?, r.get(19)?, r.get(20)?);
    Ok(match (online, runner, heard_at, draining) {
        (true, Some(_), Some(_), Some(_)) => LinkState::Draining,
        (true, Some(runner), Some(heard_at), None) => LinkState::Linked {
            runner: RunnerAddr::new(runner),
            heard_at: Ts::from_millis(heard_at),
        },
        _ => LinkState::Offline,
    })
}
