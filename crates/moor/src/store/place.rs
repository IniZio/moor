//! What a Box was started as, and the claim of a place, read with the Box.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::inbox::InboxKind;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::UserId;
use moor_core::BoxId;
use moor_schema::tables::{BoxState, Boxes, PlaceClaims};
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query, SelectStatement};

use super::db::{Db, DbExt as _, Row};
use super::init::parsed;
use super::reads::{query_boxes, SHUT};
use super::select::{box_at, place_at};
use super::{load_box, BoxRow, Store, StoreError};

/// `q`, a select from `boxes`, with each Box's claim joined; a Box no task has claimed gets nulls.
pub(super) fn join_claim(q: &mut SelectStatement) -> &mut SelectStatement {
    q.left_join(
        PlaceClaims::Table,
        Expr::col((PlaceClaims::Table, PlaceClaims::Box)).equals((Boxes::Table, Boxes::Id)),
    )
}

/// A Box's kind and claim columns as stored.
struct Stored {
    /// `work` or `place`.
    kind: String,
    /// The claim's repos as a JSON array, null for no claim.
    repos: Option<String>,
    /// The claim's prompt.
    prompt: Option<String>,
    /// When the task took the place.
    claimed_at: Option<i64>,
    /// When the claim was served.
    served_at: Option<i64>,
    /// When the first prompt left the inbox.
    steered_at: Option<i64>,
}

impl Stored {
    /// The kind these columns hold.
    ///
    /// A claim on a work Box, or a claim that names no repo, is an invalid row.
    fn kind(self) -> Result<BoxKind, StoreError> {
        let claim = self.claim()?;
        match (self.kind.as_str(), claim) {
            ("work", None) => Ok(BoxKind::Work),
            ("work", Some(_)) => Err(StoreError::Invalid("a claim row on a work Box".to_owned())),
            ("place", None) => Ok(BoxKind::Place(PlaceState::Idle)),
            ("place", Some(claim)) => Ok(BoxKind::Place(PlaceState::Claimed(claim))),
            (other, _) => Err(StoreError::Corrupt(format!("box kind `{other}`"))),
        }
    }

    /// The claim, when the Box has one.
    fn claim(&self) -> Result<Option<Claim>, StoreError> {
        let (Some(repos), Some(prompt), Some(at)) = (&self.repos, &self.prompt, self.claimed_at)
        else {
            return Ok(None);
        };
        let listed: Vec<String> = parsed("claim repos", repos, |s| serde_json::from_str(s))?;
        let mut repos = listed
            .iter()
            .map(|r| parsed("claim repo", r, ForgeRepo::parse));
        let Some(first) = repos.next().transpose()? else {
            return Err(StoreError::Invalid("a claim that names no repo".to_owned()));
        };
        Ok(Some(Claim {
            repo: first,
            also: repos.collect::<Result<_, _>>()?,
            prompt: prompt.clone(),
            at,
            served: self.served_at,
            steered: self.steered_at,
        }))
    }
}

/// The kind of the Box a read row holds.
pub(super) fn kind_from_row(row: &Row<'_>) -> Result<BoxKind, StoreError> {
    Stored {
        kind: row.get(const { box_at(Boxes::Kind) })?,
        repos: row.get(const { place_at(PlaceClaims::Repos) })?,
        prompt: row.get(const { place_at(PlaceClaims::Prompt) })?,
        claimed_at: row.get(const { place_at(PlaceClaims::ClaimedAt) })?,
        served_at: row.get(const { place_at(PlaceClaims::ServedAt) })?,
        steered_at: row.get(const { place_at(PlaceClaims::SteeredAt) })?,
    }
    .kind()
}

impl Store {
    /// What Box `id` was started as; none when no such Box exists. It reads only the kind and the
    /// claim, for a reader that asks of every event.
    pub(crate) fn box_kind(&self, id: BoxId) -> Result<Option<BoxKind>, StoreError> {
        kind_of(&self.reader()?, id)
    }

    /// `user`'s open places that no task has claimed, oldest first.
    pub(crate) fn idle_places_of(&self, user: UserId) -> Result<Vec<BoxRow>, StoreError> {
        let b = |c| (Boxes::Table, c);
        let filter = Expr::col(b(Boxes::Kind))
            .eq("place")
            .and(Expr::col(b(Boxes::User)).eq(user.get()))
            .and(Expr::col((PlaceClaims::Table, PlaceClaims::Box)).is_null())
            .and(Expr::col((BoxState::Table, BoxState::State)).is_not_in(SHUT));
        query_boxes(&self.reader()?, filter, Order::Asc, None)
    }

    /// Claims place `id` for `user` with `claim` and queues `prompt` for its agent, in one write.
    /// False, writing nothing, unless the Box is still `user`'s open place that no task has
    /// claimed: a send that lost the race for it takes another.
    pub(crate) fn claim_place(
        &self,
        id: BoxId,
        user: UserId,
        claim: &Claim,
        prompt: &str,
    ) -> Result<bool, StoreError> {
        self.check_write(id);
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        let idle = load_box(&tx, id)?.is_some_and(|row| {
            row.kind.is_idle_place()
                && row.user == Some(user)
                && !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
        });
        if !idle || !insert_claim(&tx, id, Some(user), claim)? {
            return Ok(false);
        }
        let sender = InboxKind::Prompt.as_str();
        super::inbox::insert(&tx, id, (InboxKind::Prompt, prompt, sender), self.now())?;
        tx.commit()?;
        Ok(true)
    }

    /// Records that place `id`'s claim was served at `at`; a claim served before keeps its time.
    pub(crate) fn claim_served(&self, id: BoxId, at: i64) -> Result<(), StoreError> {
        self.stamp_claim(id, PlaceClaims::ServedAt, at)
    }

    /// Records that place `id`'s question reached its agent at `at`; a claim steered before
    /// keeps its time.
    pub(crate) fn claim_steered(&self, id: BoxId, at: i64) -> Result<(), StoreError> {
        self.stamp_claim(id, PlaceClaims::SteeredAt, at)
    }

    /// Sets `column` of place `id`'s claim to `at` when it is unset.
    fn stamp_claim(&self, id: BoxId, column: PlaceClaims, at: i64) -> Result<(), StoreError> {
        self.check_write(id);
        self.writer().run(
            Query::update()
                .table(PlaceClaims::Table)
                .value(column, at)
                .and_where(Expr::col(PlaceClaims::Box).eq(id.get()))
                .and_where(Expr::col(column).is_null()),
        )?;
        Ok(())
    }
}

/// [`Store::box_kind`] on any connection, which a write's own transaction may be.
pub(super) fn kind_of(conn: &dyn Db, id: BoxId) -> Result<Option<BoxKind>, StoreError> {
    let c = |c| (PlaceClaims::Table, c);
    let mut q = Query::select();
    q.column((Boxes::Table, Boxes::Kind))
        .columns(
            [
                PlaceClaims::Repos,
                PlaceClaims::Prompt,
                PlaceClaims::ClaimedAt,
                PlaceClaims::ServedAt,
                PlaceClaims::SteeredAt,
            ]
            .map(c),
        )
        .from(Boxes::Table)
        .and_where(Expr::col((Boxes::Table, Boxes::Id)).eq(id.get()));
    join_claim(&mut q);
    conn.rows(q, |r| {
        Stored {
            kind: r.get(0)?,
            repos: r.get(1)?,
            prompt: r.get(2)?,
            claimed_at: r.get(3)?,
            served_at: r.get(4)?,
            steered_at: r.get(5)?,
        }
        .kind()
    })?
    .next()
    .transpose()
}

#[cfg(test)]
impl Store {
    /// Records `claim` on Box `id` for `user`, as a task that took the Box would.
    pub(crate) fn put_claim(
        &self,
        id: BoxId,
        user: UserId,
        claim: &Claim,
    ) -> Result<(), StoreError> {
        claimed_once(insert_claim(&self.writer(), id, Some(user), claim)?)
    }
}

/// Refuses a place that names a repo: it starts with none.
pub(super) fn check_repo(kind: &BoxKind, has_repo: bool) -> Result<(), StoreError> {
    if has_repo && matches!(kind, BoxKind::Place(_)) {
        return Err(StoreError::Invalid("a place has no repo".to_owned()));
    }
    Ok(())
}

/// Write the claim a new Box starts with, if `kind` holds one, for `user`, who started it.
pub(super) fn write_claim(
    conn: &dyn Db,
    id: BoxId,
    user: Option<UserId>,
    kind: &BoxKind,
) -> Result<(), StoreError> {
    match kind {
        BoxKind::Place(PlaceState::Claimed(claim)) => {
            claimed_once(insert_claim(conn, id, user, claim)?)
        }
        _ => Ok(()),
    }
}

/// Refuses a claim write that found the place claimed already.
fn claimed_once(written: bool) -> Result<(), StoreError> {
    if written {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the place is claimed already".to_owned(),
        ))
    }
}

/// Write the claim of place `id` for `user`; false, writing nothing, when it has one already.
fn insert_claim(
    tx: &dyn Db,
    id: BoxId,
    user: Option<UserId>,
    claim: &Claim,
) -> Result<bool, StoreError> {
    let user =
        user.ok_or_else(|| StoreError::Invalid("a claim needs the task's user".to_owned()))?;
    let repos: Vec<String> = claim.repos().map(ToString::to_string).collect();
    let repos = serde_json::to_string(&repos)?;
    let written = tx.run(
        Query::insert()
            .into_table(PlaceClaims::Table)
            .columns([
                PlaceClaims::Box,
                PlaceClaims::User,
                PlaceClaims::Repos,
                PlaceClaims::Prompt,
                PlaceClaims::ClaimedAt,
                PlaceClaims::ServedAt,
                PlaceClaims::SteeredAt,
            ])
            .values_panic([
                id.get().into(),
                user.get().into(),
                repos.into(),
                claim.prompt.as_str().into(),
                claim.at.into(),
                claim.served.into(),
                claim.steered.into(),
            ])
            .on_conflict(OnConflict::column(PlaceClaims::Box).do_nothing().to_owned()),
    )?;
    Ok(written == 1)
}
