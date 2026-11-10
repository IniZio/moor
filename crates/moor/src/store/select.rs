//! The selects the Box and event reads share.

use moor_core::BoxId;
use moor_schema::tables::{BoxHands, BoxState, Boxes, Events, Sessions};
use sea_query::{Expr, ExprTrait as _, Order, Query, SelectStatement, UpdateStatement};

use super::hands::join_current;
use super::{db, event_from_row, EventRow, StoreError};
use crate::store::db::DbExt as _;

/// The `boxes` columns a Box read takes first.
const BOX_COLS: [Boxes; 12] = [
    Boxes::Id,
    Boxes::Name,
    Boxes::Repo,
    Boxes::Issue,
    Boxes::Base,
    Boxes::Branch,
    Boxes::CreatedAt,
    Boxes::User,
    Boxes::RepoConfig,
    Boxes::AuthorName,
    Boxes::AuthorEmail,
    Boxes::OtherRepos,
];
/// The `box_state` columns a Box read takes next.
const STATE_COLS: [BoxState; 16] = [
    BoxState::Prompt,
    BoxState::State,
    BoxState::StateReason,
    BoxState::SetupFailedAt,
    BoxState::UpdatedAt,
    BoxState::IdleSince,
    BoxState::LaunchFailure,
    BoxState::StepsPendingAt,
    BoxState::Wanted,
    BoxState::WantedForce,
    BoxState::WantedReason,
    BoxState::Effect,
    BoxState::EffectAttempt,
    BoxState::EffectRun,
    BoxState::EffectAt,
    BoxState::ParkedAtWork,
];
/// The newest Session's columns a Box read takes next.
const SESSION_COLS: [Sessions; 2] = [Sessions::AgentSessionId, Sessions::State];
/// The current hands' columns a Box read ends with.
const HANDS_COLS: [BoxHands; 12] = [
    BoxHands::Inc,
    BoxHands::Provider,
    BoxHands::Handle,
    BoxHands::State,
    BoxHands::AttemptedHandle,
    BoxHands::Workdir,
    BoxHands::Egress,
    BoxHands::SleepReason,
    BoxHands::Cpus,
    BoxHands::MemMib,
    BoxHands::StateAt,
    BoxHands::WokeAt,
];

/// A const fn giving a column's position in a Box read row: its place in `$cols`, after `$skip`.
/// Called in a `const` block, a column the read does not take fails the build.
macro_rules! position {
    ($(#[$doc:meta])* $name:ident: $ty:ty, $cols:expr, $skip:expr) => {
        $(#[$doc])*
        pub(super) const fn $name(c: $ty) -> usize {
            let mut rest: &[$ty] = &$cols;
            let mut at = $skip;
            while let [first, tail @ ..] = rest {
                if *first as usize == c as usize {
                    return at;
                }
                rest = tail;
                at += 1;
            }
            panic!("not a column of the Box read")
        }
    };
}

position!(
    /// The position of a `boxes` column in a Box read row.
    box_at: Boxes, BOX_COLS, 0
);
position!(
    /// The position of a `box_state` column in a Box read row.
    state_at: BoxState, STATE_COLS, BOX_COLS.len()
);
position!(
    /// The position of a newest-Session column in a Box read row.
    session_at: Sessions, SESSION_COLS, BOX_COLS.len() + STATE_COLS.len()
);
position!(
    /// The position of a current-hands column in a Box read row.
    hands_at: BoxHands, HANDS_COLS, BOX_COLS.len() + STATE_COLS.len() + SESSION_COLS.len()
);

/// `q`, a select from `boxes`, with each Box's `box_state` row joined.
pub(super) fn join_state(q: &mut SelectStatement) -> &mut SelectStatement {
    q.inner_join(
        BoxState::Table,
        Expr::col((BoxState::Table, BoxState::Box)).equals((Boxes::Table, Boxes::Id)),
    )
}

/// An UPDATE of Box `id`'s `box_state` row.
pub(super) fn state_update(id: BoxId) -> UpdateStatement {
    Query::update()
        .table(BoxState::Table)
        .and_where(Expr::col(BoxState::Box).eq(id.get()))
        .take()
}

/// The Box reads' columns, in the order the positions above give: the state columns are the
/// Box's `box_state` row's, the session columns its newest Session's, the hands columns its
/// current hands'.
pub(super) fn box_select() -> SelectStatement {
    let b = |c| (Boxes::Table, c);
    let s = |c| (Sessions::Table, c);
    let newest = Query::select()
        .expr(Expr::col(("newest", Sessions::Ordinal)).max())
        .from_as(Sessions::Table, "newest")
        .and_where(Expr::col(("newest", Sessions::Box)).equals(b(Boxes::Id)))
        .take();
    let mut q = Query::select()
        .columns(BOX_COLS.map(b))
        .columns(STATE_COLS.map(|c| (BoxState::Table, c)))
        .columns(SESSION_COLS.map(s))
        .columns(HANDS_COLS.map(|c| (BoxHands::Table, c)))
        .from(Boxes::Table)
        .left_join(
            Sessions::Table,
            Expr::col(s(Sessions::Box))
                .equals(b(Boxes::Id))
                .and(Expr::col(s(Sessions::Ordinal)).eq(db::sub(newest))),
        )
        .take();
    join_state(&mut q);
    join_current(&mut q);
    q
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
