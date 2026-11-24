//! What a Box was started as, and the claim of a place, read with the Box.

use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::UserId;
use moor_core::BoxId;
use moor_schema::tables::{Boxes, PlaceClaims};
use sea_query::{Expr, ExprTrait as _, Query, SelectStatement};

use super::db::{Db, DbExt as _, Row};
use super::init::parsed;
use super::select::{box_at, place_at};
use super::{Store, StoreError};

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
        insert_claim(&self.writer(), id, Some(user), claim)
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
        BoxKind::Place(PlaceState::Claimed(claim)) => insert_claim(conn, id, user, claim),
        _ => Ok(()),
    }
}

/// Write the claim of place `id` for `user`, who started it.
fn insert_claim(
    tx: &dyn Db,
    id: BoxId,
    user: Option<UserId>,
    claim: &Claim,
) -> Result<(), StoreError> {
    let user =
        user.ok_or_else(|| StoreError::Invalid("a claim needs the task's user".to_owned()))?;
    let repos: Vec<String> = claim.repos().map(ToString::to_string).collect();
    let repos = serde_json::to_string(&repos)?;
    tx.run(
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
            ]),
    )?;
    Ok(())
}
