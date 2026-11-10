//! The wanted action and the effect in flight, as a Box read row holds them.

use moor_core::domain::decision::{Effect, Intent, Wanted};
use moor_core::RunId;
use moor_schema::tables::BoxState;

use super::db::Row;
use super::select::state_at;
use super::StoreError;

/// The wanted action a Box read row holds.
pub(super) fn wanted_from(row: &Row<'_>) -> Result<Option<Wanted>, StoreError> {
    let Some(kind) = row.get::<_, Option<String>>(const { state_at(BoxState::Wanted) })? else {
        return Ok(None);
    };
    let force = row
        .get::<_, Option<i64>>(const { state_at(BoxState::WantedForce) })?
        .is_some_and(|f| f != 0);
    let reason = row.get(const { state_at(BoxState::WantedReason) })?;
    Wanted::from_columns(&kind, force, reason)
        .map(Some)
        .ok_or_else(|| StoreError::Corrupt(format!("wanted `{kind}`")))
}

/// The effect in flight a Box read row holds.
pub(super) fn effect_from(row: &Row<'_>) -> Result<Option<Effect>, StoreError> {
    let Some(kind) = row.get::<_, Option<String>>(const { state_at(BoxState::Effect) })? else {
        return Ok(None);
    };
    let run = row
        .get::<_, Option<i64>>(const { state_at(BoxState::EffectRun) })?
        .map(RunId::new);
    Effect::from_columns(&kind, run)
        .map(Some)
        .ok_or_else(|| StoreError::Corrupt(format!("effect `{kind}`")))
}

/// Rebuild the intent from its kind and run columns.
pub(super) fn intent_from(
    kind: Option<String>,
    run: Option<i64>,
) -> Result<Option<Intent>, StoreError> {
    let Some(kind) = kind else { return Ok(None) };
    match kind.as_str() {
        "provisioning" => Ok(Some(Intent::Provisioning)),
        "setup" => Ok(Some(Intent::Setup)),
        "closing" => Ok(Some(Intent::Closing)),
        "running" => {
            let run =
                run.ok_or_else(|| StoreError::Corrupt("running intent without run".into()))?;
            Ok(Some(Intent::Running {
                run: RunId::new(run),
            }))
        }
        other => Err(StoreError::Corrupt(format!("intent kind `{other}`"))),
    }
}
