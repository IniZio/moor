//! The mapping from a Box read's row to a [`BoxRow`].

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName};
use moor_schema::tables::{BoxState, Boxes, Sessions};

use super::db::Row;
use super::hands::hands_from_row;
use super::in_flight::{effect_from, wanted_from};
use super::init::{parsed, parsed_opt};
use super::repo::repo_from_row;
use super::{place, select, to_u32, BoxRow, StoreError};

/// Map a `BOX_SELECT` row to a `BoxRow`.
pub(super) fn box_from_row(row: &Row<'_>) -> Result<BoxRow, StoreError> {
    use select::{box_at as at, state_at};
    let name: String = row.get(const { at(Boxes::Name) })?;
    let lifecycle: String = row.get(const { state_at(BoxState::State) })?;
    let session: Option<String> = row.get(const { select::session_at(Sessions::State) })?;
    let session = match session {
        Some(raw) => parsed_opt("session state", &raw, SessionState::parse)?,
        None => SessionState::Starting,
    };
    let lifecycle = parsed_opt("box state", &lifecycle, BoxLifecycle::parse)?;
    let ts = |at| Ok::<_, StoreError>(row.get::<_, Option<i64>>(at)?.map(Ts::from_millis));
    Ok(BoxRow {
        id: BoxId::new(row.get(const { at(Boxes::Id) })?),
        name: parsed("box name", &name, BoxName::parse_allow_bare)?,
        repo: repo_from_row(row)?,
        user: row
            .get::<_, Option<i64>>(const { at(Boxes::User) })?
            .map(UserId::new),
        issue: row.get(const { at(Boxes::Issue) })?,
        prompt: row.get(const { state_at(BoxState::Prompt) })?,
        base: row.get(const { at(Boxes::Base) })?,
        base_ref: row.get(const { at(Boxes::BaseRef) })?,
        branch: row.get(const { at(Boxes::Branch) })?,
        agent_session_id: row.get(const { select::session_at(Sessions::AgentSessionId) })?,
        lifecycle,
        session,
        state_reason: row.get(const { state_at(BoxState::StateReason) })?,
        wanted: wanted_from(row)?,
        effect: effect_from(row)?,
        effect_attempt: row
            .get::<_, Option<i64>>(const { state_at(BoxState::EffectAttempt) })?
            .map_or(Ok(0), |n| to_u32("effect attempt", n))?,
        effect_at: ts(const { state_at(BoxState::EffectAt) })?,
        parked_at_work: ts(const { state_at(BoxState::ParkedAtWork) })?,
        setup_failed_at: ts(const { state_at(BoxState::SetupFailedAt) })?,
        steps_pending_at: ts(const { state_at(BoxState::StepsPendingAt) })?,
        created_at: Ts::from_millis(row.get(const { at(Boxes::CreatedAt) })?),
        updated_at: Ts::from_millis(row.get(const { state_at(BoxState::UpdatedAt) })?),
        idle_since: ts(const { state_at(BoxState::IdleSince) })?,
        launch_failure: row
            .get::<_, Option<String>>(const { state_at(BoxState::LaunchFailure) })?
            .map(|j| serde_json::from_str(&j))
            .transpose()?,
        hands: hands_from_row(row)?,
        kind: place::kind_from_row(row)?,
        task: row
            .get::<_, Option<i64>>(const { at(Boxes::Task) })?
            .map(BoxId::new),
        no_setup: row.get::<_, i64>(const { at(Boxes::NoSetup) })? != 0,
    })
}
