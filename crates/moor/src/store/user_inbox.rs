//! Each person's inbox rows: one per event that needed them, written in the event's own
//! transaction, with the read mark the person sets. Whether the wait an item stands for is over
//! is decided when the list is built, by [`moor_core::inbox`].

use moor_core::domain::event::Event;
use moor_core::inbox::{entry_of, InboxKind};
use moor_core::progress::title_of;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use moor_schema::tables::{BoxState, Boxes, Events, UserInbox, WorkflowRuns};
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use super::db::{DbExt as _, OptionalExtension as _, Tx};
use super::{Store, StoreError};

/// The `outcome` of a run that waits for its owner, as `workflow_runs` stores it.
const WAITING: &str = "waiting";

/// The events that ask a question or end one. A turn ending is not among them: the agent's turn
/// ends right when it asks, and the question stands.
const QUESTION_MARKERS: [&str; 4] = ["needs_input", "steered", "working", "busy"];

/// One inbox row with the Box it is about and the event's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserInboxRow {
    /// The item's id.
    pub(crate) id: i64,
    /// The Box.
    pub(crate) box_id: BoxId,
    /// The Box's name.
    pub(crate) box_name: String,
    /// The Box's repo.
    pub(crate) repo: Option<String>,
    /// Why it is there.
    pub(crate) kind: InboxKind,
    /// The event it was written for.
    pub(crate) event: i64,
    /// What it is about.
    pub(crate) title: String,
    /// The question or reason.
    pub(crate) text: String,
    /// When the event was written, in unix milliseconds.
    pub(crate) created_at: i64,
    /// Whether the person marked it read.
    pub(crate) read: bool,
    /// The event's payload, as stored.
    pub(crate) payload: String,
}

/// Adds the inbox row for event `event` of Box `id` when the event is one that needs its
/// owner; every other event returns before any query. Written twice for one event, it stays one.
pub(super) fn note(
    tx: &Tx<'_>,
    id: BoxId,
    (event, e): (i64, &Event),
    now: Ts,
) -> Result<(), StoreError> {
    if InboxKind::of_event(e.kind()).is_none() {
        return Ok(());
    }
    let owner: Option<(Option<i64>, String, Option<String>)> = tx
        .row(
            Query::select()
                .column((Boxes::Table, Boxes::User))
                .column((Boxes::Table, Boxes::Name))
                .column((BoxState::Table, BoxState::Prompt))
                .from(Boxes::Table)
                .left_join(
                    BoxState::Table,
                    Expr::col((BoxState::Table, BoxState::Box)).equals((Boxes::Table, Boxes::Id)),
                )
                .and_where(Expr::col((Boxes::Table, Boxes::Id)).eq(id.get())),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((Some(user), name, prompt)) = owner else {
        return Ok(());
    };
    let title = title_of(prompt.as_deref(), &name);
    let Some(entry) = entry_of(e.kind(), &e.payload(), &title) else {
        return Ok(());
    };
    tx.run(
        Query::insert()
            .into_table(UserInbox::Table)
            .columns([
                UserInbox::User,
                UserInbox::Box,
                UserInbox::Event,
                UserInbox::Kind,
                UserInbox::Title,
                UserInbox::Text,
                UserInbox::CreatedAt,
            ])
            .values_panic([
                user.into(),
                id.get().into(),
                event.into(),
                entry.kind.as_str().into(),
                entry.title.into(),
                entry.text.into(),
                now.as_millis().into(),
            ])
            .on_conflict(OnConflict::column(UserInbox::Event).do_nothing().to_owned()),
    )?;
    Ok(())
}

impl Store {
    /// The newest `limit` inbox rows of `user`, newest first.
    pub(crate) fn user_inbox(
        &self,
        user: UserId,
        limit: u32,
    ) -> Result<Vec<UserInboxRow>, StoreError> {
        let i = |c| (UserInbox::Table, c);
        let rows = self.reader()?.rows(
            Query::select()
                .column(i(UserInbox::Id))
                .column(i(UserInbox::Box))
                .column((Boxes::Table, Boxes::Name))
                .column((Boxes::Table, Boxes::Repo))
                .column(i(UserInbox::Kind))
                .column(i(UserInbox::Event))
                .column(i(UserInbox::Title))
                .column(i(UserInbox::Text))
                .column(i(UserInbox::CreatedAt))
                .column(i(UserInbox::ReadAt))
                .column((Events::Table, Events::Payload))
                .from(UserInbox::Table)
                .inner_join(
                    Boxes::Table,
                    Expr::col((Boxes::Table, Boxes::Id)).equals(i(UserInbox::Box)),
                )
                .inner_join(
                    Events::Table,
                    Expr::col((Events::Table, Events::Id)).equals(i(UserInbox::Event)),
                )
                .and_where(Expr::col(i(UserInbox::User)).eq(user.get()))
                .order_by(i(UserInbox::Id), Order::Desc)
                .limit(u64::from(limit)),
            |r| {
                let kind: String = r.get(4)?;
                Ok::<_, StoreError>(UserInboxRow {
                    id: r.get(0)?,
                    box_id: BoxId::new(r.get(1)?),
                    box_name: r.get(2)?,
                    repo: r.get(3)?,
                    kind: InboxKind::parse(&kind)
                        .ok_or_else(|| StoreError::Corrupt(format!("user_inbox kind `{kind}`")))?,
                    event: r.get(5)?,
                    title: r.get(6)?,
                    text: r.get(7)?,
                    created_at: r.get(8)?,
                    read: r.get::<_, Option<i64>>(9)?.is_some(),
                    payload: r.get(10)?,
                })
            },
        )?;
        rows.collect()
    }

    /// The id of the newest event of Box `id` that either asks the person or ends a question: a
    /// `needs_input`, an answer sent, or the agent working again.
    pub(crate) fn newest_question_marker(&self, id: BoxId) -> Result<Option<i64>, StoreError> {
        Ok(self
            .reader()?
            .row(
                Query::select()
                    .column(Events::Id)
                    .from(Events::Table)
                    .and_where(Expr::col(Events::Box).eq(id.get()))
                    .and_where(Expr::col(Events::Kind).is_in(QUESTION_MARKERS))
                    .order_by(Events::Id, Order::Desc)
                    .limit(1),
                |r| r.get::<_, i64>(0),
            )
            .optional()?)
    }

    /// Marks the unread items `ids` of `user` read, or every unread item when `ids` is empty.
    /// The number it marked; another person's id marks nothing.
    pub(crate) fn mark_inbox_read(&self, user: UserId, ids: &[i64]) -> Result<usize, StoreError> {
        let now = self.now().as_millis();
        let mut update = Query::update();
        update
            .table(UserInbox::Table)
            .value(UserInbox::ReadAt, now)
            .and_where(Expr::col(UserInbox::User).eq(user.get()))
            .and_where(Expr::col(UserInbox::ReadAt).is_null());
        if !ids.is_empty() {
            update.and_where(Expr::col(UserInbox::Id).is_in(ids.iter().copied()));
        }
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        let changed = tx.run(update)?;
        tx.commit()?;
        drop(writer);
        Ok(changed)
    }

    /// Whether run `run` waits at an approve step since `since`, in unix milliseconds. A run
    /// that answered and waits at the same step again waits since a later moment.
    pub(crate) fn run_waits_since(&self, run: i64, since: i64) -> Result<bool, StoreError> {
        Ok(self
            .reader()?
            .row(
                Query::select()
                    .column(WorkflowRuns::Id)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Id).eq(run))
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(WAITING))
                    .and_where(Expr::col(WorkflowRuns::WaitingSince).eq(since)),
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }
}
