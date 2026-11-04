//! The selects the Box and event reads share.

use moor_core::BoxId;
use moor_schema::tables::{Boxes, Events, Sessions};
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement};

use super::{db, event_from_row, EventRow, StoreError};
use crate::store::db::DbExt as _;

/// The Box columns a Box read starts with.
const BOX_HEAD: [Boxes; 7] = [
    Boxes::Id,
    Boxes::Name,
    Boxes::Repo,
    Boxes::Issue,
    Boxes::Prompt,
    Boxes::Base,
    Boxes::Branch,
];
/// The Box columns between the newest Session's ids and its state.
const BOX_MIDDLE: [Boxes; 19] = [
    Boxes::State,
    Boxes::StateReason,
    Boxes::IntentKind,
    Boxes::IntentPid,
    Boxes::IntentRun,
    Boxes::IntentAt,
    Boxes::SetupFailedAt,
    Boxes::CreatedAt,
    Boxes::UpdatedAt,
    Boxes::Provider,
    Boxes::Handle,
    Boxes::DriverState,
    Boxes::AttemptedHandle,
    Boxes::Workdir,
    Boxes::Egress,
    Boxes::SleepReason,
    Boxes::DriverUpdatedAt,
    Boxes::IdleSince,
    Boxes::LaunchFailure,
];
/// The Box columns a Box read ends with.
const BOX_TAIL: [Boxes; 8] = [
    Boxes::User,
    Boxes::RepoConfig,
    Boxes::AuthorName,
    Boxes::AuthorEmail,
    Boxes::Cpus,
    Boxes::MemMib,
    Boxes::OtherRepos,
    Boxes::StepsPendingAt,
];

/// The Box reads' columns, in the order [`super::box_from_row`] reads them; the session columns are
/// the Box's newest Session's.
pub(super) fn box_select() -> SelectStatement {
    let b = |c| (Boxes::Table, c);
    let s = |c| (Sessions::Table, c);
    let newest = Query::select()
        .expr(Expr::col(("newest", Sessions::Ordinal)).max())
        .from_as(Sessions::Table, "newest")
        .and_where(Expr::col(("newest", Sessions::Box)).equals(b(Boxes::Id)))
        .take();
    Query::select()
        .columns(BOX_HEAD.map(b))
        .columns([Sessions::WorkspaceId, Sessions::AgentSessionId].map(s))
        .columns(BOX_MIDDLE.map(b))
        .column(s(Sessions::State))
        .columns(BOX_TAIL.map(b))
        .from(Boxes::Table)
        .left_join(
            Sessions::Table,
            Expr::col(s(Sessions::Box))
                .equals(b(Boxes::Id))
                .and(Expr::col(s(Sessions::Ordinal)).eq(db::sub(newest))),
        )
        .take()
}

/// The id of Box `id`'s newest Session.
pub(super) fn newest_session(id: BoxId) -> SelectStatement {
    Query::select()
        .column(Sessions::Id)
        .from(Sessions::Table)
        .and_where(Expr::col(Sessions::Box).eq(id.get()))
        .order_by(Sessions::Ordinal, Order::Desc)
        .limit(1)
        .take()
}

/// The columns [`event_from_row`] reads, in order.
pub(super) const EVENT_COLUMNS: [Events; 5] = [
    Events::Id,
    Events::Box,
    Events::Kind,
    Events::Payload,
    Events::Ts,
];

/// Box `id`'s events, with the columns [`event_from_row`] reads.
pub(super) fn events_of(id: BoxId) -> SelectStatement {
    Query::select()
        .columns(EVENT_COLUMNS)
        .from(Events::Table)
        .and_where(Expr::col(Events::Box).eq(id.get()))
        .take()
}

/// The newest of the events `q` selects; none when it selects none.
pub(super) fn newest_event(
    conn: &dyn db::Db,
    q: &mut SelectStatement,
) -> Result<Option<EventRow>, StoreError> {
    let q = q.order_by(Events::Id, Order::Desc).limit(1);
    conn.rows(q, event_from_row)?.next().transpose()
}

/// A row count as a `LIMIT`, capped where the database's signed integers end.
pub(super) fn limit(n: usize) -> u64 {
    Ord::min(
        u64::try_from(n).unwrap_or(u64::MAX),
        i64::MAX.unsigned_abs(),
    )
}
