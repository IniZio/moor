//! What moor wants a Box to end as, and the effect in flight, follow the store's rules whichever
//! op writes first: a weaker wanted action never replaces a stronger one, the lifecycle step that
//! fulfils one clears it, and an effect begins only when none is in flight.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted, WantedKind};
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, RunId};

use super::hands_fence::ready_box;
use super::open;
use crate::store::{Store, StoreError};

fn close(force: bool) -> Wanted {
    Wanted::Close {
        force,
        reason: None,
    }
}

fn suspend() -> Wanted {
    Wanted::Suspend {
        reason: "memory".to_owned(),
    }
}

fn want(store: &Store, id: BoxId, change: WantChange) {
    store.apply(Decision::new(id).want(change)).unwrap();
}

fn wanted(store: &Store, id: BoxId) -> Option<Wanted> {
    store.box_row(id).unwrap().unwrap().wanted
}

/// The effect in flight and its attempt.
fn effect(store: &Store, id: BoxId) -> (Option<Effect>, u32) {
    let row = store.box_row(id).unwrap().unwrap();
    (row.effect, row.effect_attempt)
}

#[test]
fn a_suspend_raised_over_a_close_leaves_the_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Raise(suspend()));
    want(&store, id, WantChange::Raise(Wanted::Sleep));

    assert_eq!(wanted(&store, id), Some(close(false)));
}

#[test]
fn a_close_raised_over_a_suspend_outlives_the_suspend_it_raced() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(suspend()));
    want(&store, id, WantChange::Raise(close(true)));

    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Suspend {
            reason: "memory".to_owned(),
        }))
        .unwrap();

    assert_eq!(wanted(&store, id), Some(close(true)));
}

#[test]
fn a_forced_close_replaces_one_that_was_not_forced() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Raise(close(true)));
    want(&store, id, WantChange::Raise(close(false)));

    assert_eq!(wanted(&store, id), Some(close(true)));
}

#[test]
fn done_clears_only_its_own_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    want(&store, id, WantChange::Done(WantedKind::Suspend));
    assert_eq!(wanted(&store, id), Some(close(false)));

    want(&store, id, WantChange::Done(WantedKind::Close));
    assert_eq!(wanted(&store, id), None);
}

#[test]
fn closing_the_box_clears_what_it_wanted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap();

    assert_eq!(wanted(&store, id), None);
}

#[test]
fn begin_over_an_effect_in_flight_is_invalid_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let begin = |e| Decision::new(id).effect(EffectChange::Begin(e));
    store.apply(begin(Effect::Setup)).unwrap();

    let got = store.apply(begin(Effect::Launch));

    assert!(matches!(got, Err(StoreError::Invalid(_))), "{got:?}");
    assert_eq!(effect(&store, id), (Some(Effect::Setup), 1));
}

#[test]
fn again_bumps_the_attempt_and_end_clears_the_effect() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let run = Effect::Run { run: RunId::new(7) };
    store
        .apply(Decision::new(id).effect(EffectChange::Begin(run)))
        .unwrap();

    store
        .apply(Decision::new(id).effect(EffectChange::Again))
        .unwrap();
    assert_eq!(effect(&store, id), (Some(run), 2));

    store
        .apply(Decision::new(id).effect(EffectChange::End))
        .unwrap();
    assert_eq!(effect(&store, id), (None, 0));
}

#[test]
fn again_with_no_effect_in_flight_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);

    let got = store.apply(Decision::new(id).effect(EffectChange::Again));

    assert!(matches!(got, Err(StoreError::Invalid(_))), "{got:?}");
}

/// Applies lifecycle input `input` to Box `id`.
fn step(store: &Store, id: BoxId, input: BoxLifecycleInput) {
    store.apply(Decision::new(id).lifecycle(input)).unwrap();
}

fn sleep() -> BoxLifecycleInput {
    BoxLifecycleInput::Sleep {
        auto: true,
        reason: None,
    }
}

/// A Ready Box whose agent is quiet, so it may sleep.
fn quiet_box(store: &Store) -> BoxId {
    let id = ready_box(store);
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    id
}

#[test]
fn a_sleep_never_clears_a_wanted_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = quiet_box(&store);
    want(&store, id, WantChange::Raise(close(false)));

    step(&store, id, sleep());

    assert_eq!(wanted(&store, id), Some(close(false)));
}

#[test]
fn a_sleep_clears_a_wanted_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = quiet_box(&store);
    want(&store, id, WantChange::Raise(Wanted::Sleep));

    step(&store, id, sleep());

    assert_eq!(wanted(&store, id), None);
}

#[test]
fn a_suspend_clears_a_wanted_suspend() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(suspend()));

    step(
        &store,
        id,
        BoxLifecycleInput::Suspend {
            reason: "memory".to_owned(),
        },
    );

    assert_eq!(wanted(&store, id), None);
}

#[test]
fn a_close_clears_a_wanted_suspend() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    want(&store, id, WantChange::Raise(suspend()));

    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Close { reason: None })
                .session(SessionInput::End),
        )
        .unwrap();

    assert_eq!(wanted(&store, id), None);
}

#[test]
fn a_rejection_clears_a_wanted_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = super::new_box(&store, "r");
    want(&store, id, WantChange::Raise(close(false)));

    step(
        &store,
        id,
        BoxLifecycleInput::Rejected {
            reason: "no".to_owned(),
        },
    );

    assert_eq!(wanted(&store, id), None);
}

/// The stored wanted reason and time, and the effect time, of Box `id`.
fn stamps(store: &Store, id: BoxId) -> (Option<String>, Option<i64>, Option<i64>) {
    use moor_schema::tables::BoxState;
    use sea_query::{Expr, ExprTrait as _, Query};

    use crate::store::db::DbExt as _;
    let q = Query::select()
        .columns([
            BoxState::WantedReason,
            BoxState::WantedAt,
            BoxState::EffectAt,
        ])
        .from(BoxState::Table)
        .and_where(Expr::col(BoxState::Box).eq(id.get()))
        .take();
    store
        .reader()
        .unwrap()
        .row(q, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
}

#[test]
fn a_wanted_suspend_keeps_its_reason_and_time_and_an_ended_effect_its_time_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    let now = {
        use moor_core::seam::clock::Clock as _;
        clock.now().as_millis()
    };
    want(&store, id, WantChange::Raise(suspend()));
    store
        .apply(Decision::new(id).effect(EffectChange::Begin(Effect::Setup)))
        .unwrap();
    assert_eq!(
        stamps(&store, id),
        (Some("memory".to_owned()), Some(now), Some(now))
    );

    store
        .apply(Decision::new(id).effect(EffectChange::End))
        .unwrap();

    assert_eq!(stamps(&store, id).2, None);
}

/// Starts a run on Box `id`, as `moor run` does, and lets its claim go.
fn run_on(store: &Store, id: BoxId) -> RunId {
    let (run, _claim) = store
        .begin_run(crate::store::NewRun {
            box_id: id,
            argv: vec!["make".to_owned()],
            record: None,
            sha: None,
            pid: 1,
        })
        .unwrap();
    run
}

#[test]
fn a_launch_wins_the_one_effect_slot_over_a_run_whichever_starts_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    let launch = || Decision::new(id).effect(EffectChange::Next(Effect::Launch));
    store.apply(launch()).unwrap();

    let during = run_on(&store, id);
    assert_eq!(
        effect(&store, id).0,
        Some(Effect::Launch),
        "a run never displaces a launch"
    );
    store
        .finish_run(during, crate::store::RunOutcome::ended(Some(0), false))
        .unwrap();
    assert_eq!(effect(&store, id).0, Some(Effect::Launch));

    store
        .apply(Decision::new(id).effect(EffectChange::End))
        .unwrap();
    let before = run_on(&store, id);
    assert_eq!(effect(&store, id).0, Some(Effect::Run { run: before }));
    store.apply(launch()).unwrap();
    store
        .finish_run(before, crate::store::RunOutcome::ended(Some(0), false))
        .unwrap();
    assert_eq!(
        effect(&store, id).0,
        Some(Effect::Launch),
        "the run's end leaves the launch"
    );
}

impl Store {
    /// Deletes every event of Box `id`, and the webhook deliveries queued for them, so a test
    /// proves a decision reads no history.
    pub(crate) fn forget_events(&self, id: BoxId) {
        use moor_schema::tables::{Events, WebhookDeliveries};
        use sea_query::{Expr, ExprTrait as _, Query};

        use crate::store::db::DbExt as _;
        let of_box = Query::select()
            .column(Events::Id)
            .from(Events::Table)
            .and_where(Expr::col(Events::Box).eq(id.get()))
            .take();
        let conn = self.writer();
        conn.run(
            Query::delete()
                .from_table(WebhookDeliveries::Table)
                .and_where(Expr::col(WebhookDeliveries::Event).in_subquery(of_box)),
        )
        .unwrap();
        conn.run(
            Query::delete()
                .from_table(Events::Table)
                .and_where(Expr::col(Events::Box).eq(id.get())),
        )
        .unwrap();
        drop(conn);
    }
}
