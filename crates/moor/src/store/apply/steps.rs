//! Steps a Box and its primary Session through their own machines for one decision.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, StepError};
use moor_core::domain::box_steps;
use moor_core::domain::decision::{BoxTarget, Decision, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use moor_schema::tables::{BoxRefs, BoxState, Boxes, Sessions};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query, Value};

use super::sql::name_taken;
use crate::store::db::{self, DbExt as _, Tx};
use crate::store::select::state_update;
use crate::store::{load_box, sessions::live_parent, HandsRow, StoreError};

/// The Box and primary Session states a step leaves behind.
pub(super) type Parts = (BoxLifecycle, SessionState);

/// A decision resolved against its Box: the row, the stepped states and the states it left.
pub(super) struct Stepped {
    /// The Box written.
    pub(super) id: BoxId,
    /// The Box row before the step, if it existed.
    pub(super) existing: Option<HandsRow>,
    /// What the writer stores.
    pub(super) parts: Parts,
    /// The states before the step; `None` for a fresh Box.
    pub(super) from: Option<Parts>,
}

/// Resolve the Box (inserting a new one) and step its lifecycle and primary Session.
///
/// The parts are what the writer stores.
pub(super) fn step_box(tx: &Tx<'_>, now: Ts, d: &Decision) -> Result<Stepped, StoreError> {
    let (l, s) = (d.lifecycle_input.as_ref(), d.session_input.as_ref());
    match &d.target {
        BoxTarget::New(new) => {
            let Some(BoxLifecycleInput::Insert) = l else {
                return Err(StoreError::Invalid("new Box needs the insert input".into()));
            };
            let parts = (
                BoxLifecycle::initial(&BoxLifecycleInput::Insert)?,
                SessionState::initial(&SessionInput::Open)?,
            );
            let id = insert_box(tx, new, parts.0, (d.parent_session, d.requester), now)?;
            let branch = if d.branch_by_id {
                Some(moor_core::box_key::branch(id, &new.name))
            } else {
                new.spec.branch.clone()
            };
            if let Some(branch) = branch {
                write_branch(tx, id, &branch)?;
            }
            Ok(Stepped {
                id,
                existing: None,
                parts,
                from: None,
            })
        }
        BoxTarget::Existing(id) => {
            let row = load_box(tx, *id)?.ok_or(StoreError::NotFound(*id))?;
            let (lifecycle, session) = (row.lifecycle, row.session);
            let parts = if l.is_some() || s.is_some() {
                let others = read_others(tx, *id)?;
                box_steps::apply(lifecycle, session, &others, l, s).map_err(|e| match e {
                    StepError::Illegal(i) => StoreError::Illegal(i),
                    StepError::SleepBlocked { busy } => StoreError::SleepBlocked { busy },
                })?
            } else {
                (lifecycle, session)
            };
            Ok(Stepped {
                id: *id,
                existing: row.hands,
                parts,
                from: Some((lifecycle, session)),
            })
        }
    }
}

/// States of the Box's Sessions other than the current one.
fn read_others(tx: &Tx<'_>, id: BoxId) -> Result<Vec<SessionState>, StoreError> {
    let newest = Query::select()
        .expr(Expr::col(Sessions::Ordinal).max())
        .from(Sessions::Table)
        .and_where(Expr::col(Sessions::Box).eq(id.get()))
        .take();
    let rows = tx.rows(
        Query::select()
            .column(Sessions::State)
            .from(Sessions::Table)
            .and_where(Expr::col(Sessions::Box).eq(id.get()))
            .and_where(Expr::col(Sessions::Ordinal).lt(db::sub(newest))),
        |r| r.get::<_, String>(0),
    )?;
    rows.map(|s| {
        let s = s?;
        SessionState::parse(&s)
            .ok_or_else(|| StoreError::Invalid(format!("unknown Session state {s}")))
    })
    .collect()
}

/// Insert a fresh Box row for `requester` with its primary Session under `parent_session`. A
/// primary Session with no parent is a root Session and records the requester too.
fn insert_box(
    tx: &Tx<'_>,
    new: &NewBox,
    lifecycle: BoxLifecycle,
    (parent_session, requester): (Option<i64>, Option<UserId>),
    now: Ts,
) -> Result<BoxId, StoreError> {
    if let Some(p) = parent_session {
        live_parent(tx, p)?;
    }
    let row = box_columns(new, requester, now)?;
    let id: i64 = tx.row(
        Query::insert()
            .into_table(Boxes::Table)
            .columns(row.iter().map(|(c, _)| *c))
            .values_panic(row.into_iter().map(|(_, v)| v.into()))
            .returning_col(Boxes::Id),
        |r| r.get(0),
    )?;
    tx.run(
        Query::insert()
            .into_table(BoxState::Table)
            .columns([
                BoxState::Box,
                BoxState::State,
                BoxState::OpenName,
                BoxState::Prompt,
                BoxState::UpdatedAt,
            ])
            .values_panic([
                id.into(),
                lifecycle.as_str().into(),
                new.name.as_str().into(),
                new.spec.prompt.as_deref().into(),
                now.as_millis().into(),
            ]),
    )
    .map_err(|e| name_taken(e, new))?;
    tx.run(
        Query::insert()
            .into_table(Sessions::Table)
            .columns([
                Sessions::Box,
                Sessions::Ordinal,
                Sessions::State,
                Sessions::StartedAt,
                Sessions::ParentSession,
                Sessions::User,
            ])
            .values_panic([
                id.into(),
                1.into(),
                SessionState::Starting.as_str().into(),
                now.as_millis().into(),
                parent_session.into(),
                requester
                    .filter(|_| parent_session.is_none())
                    .map(UserId::get)
                    .into(),
            ]),
    )?;
    Ok(BoxId::new(id))
}

/// Name Box `id`'s branch, which the open-branch index refuses while another live Box has it.
fn write_branch(tx: &Tx<'_>, id: BoxId, branch: &str) -> Result<(), StoreError> {
    tx.run(
        Query::update()
            .table(Boxes::Table)
            .value(Boxes::Branch, branch)
            .and_where(Expr::col(Boxes::Id).eq(id.get())),
    )?;
    tx.run(state_update(id).value(BoxState::OpenBranch, branch))?;
    tx.run(
        Query::insert()
            .into_table(BoxRefs::Table)
            .columns([BoxRefs::Box, BoxRefs::Kind, BoxRefs::Value])
            .values_panic([id.get().into(), "branch".into(), branch.into()])
            .on_conflict(OnConflict::new().do_nothing().to_owned()),
    )?;
    Ok(())
}

/// The columns of a fresh Box row, each with its value.
fn box_columns(
    new: &NewBox,
    requester: Option<UserId>,
    now: Ts,
) -> Result<[(Boxes, Value); 11], StoreError> {
    let repo = new.repo.as_ref();
    let labels = serde_json::to_string(&new.spec.labels)
        .map_err(|e| StoreError::Corrupt(format!("node labels: {e}")))?;
    Ok([
        (Boxes::Name, new.name.as_str().into()),
        (Boxes::Repo, repo.map(|r| r.repo.to_string()).into()),
        (
            Boxes::RepoConfig,
            repo.and_then(|r| r.moor_yaml.as_deref()).into(),
        ),
        (
            Boxes::AuthorName,
            repo.map(|r| r.author.name.as_str()).into(),
        ),
        (
            Boxes::AuthorEmail,
            repo.map(|r| r.author.email.as_str()).into(),
        ),
        (
            Boxes::OtherRepos,
            repo.and_then(crate::store::repo::others_column).into(),
        ),
        (Boxes::Issue, new.spec.issue.as_deref().into()),
        (Boxes::Base, new.spec.base.as_deref().into()),
        (Boxes::NodeLabels, labels.into()),
        (Boxes::User, requester.map(UserId::get).into()),
        (Boxes::CreatedAt, now.as_millis().into()),
    ])
}
