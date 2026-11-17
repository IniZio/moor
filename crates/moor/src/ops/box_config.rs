//! A Box's config override: `box-config-set`, and the checks a start, a workflow and a set make
//! of the text.
//!
//! The Box's owner and the operator may set it, which [`super::authz`] decides from the Box the
//! request names. Every read of the Box's config lays the stored text over the repo's, so a set
//! reaches the next `exec`, `run`, setup line and teardown, and what else reads the config.

use moor_core::config::{override_text, RepoOverride};
use moor_core::request::BoxConfigSetReq;
use moor_core::view::box_config::BoxConfigValue;
use moor_core::{ErrorKind, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;

/// The most text an override may hold, as much as a repo's stored config.
const TEXT_CAP: usize = 64 * 1024;

/// Refuses an override that is too long or does not parse as a repo config, so a bad one fails
/// when it is given and not when a schedule fires or a Box next reads its config.
///
/// # Errors
/// Usage for text over the cap or that does not parse.
pub(crate) fn check(text: Option<&str>) -> Result<(), CommandError> {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Ok(());
    };
    if text.len() > TEXT_CAP {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("the config is over {TEXT_CAP} bytes"),
        ));
    }
    RepoOverride::parse("box config", text)
        .map(drop)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))
}

/// Refuses a start of open Box `row` that carries a config other than the Box's own, rather than
/// dropping it: an open Box keeps the config it has until `moor box-config` replaces it.
///
/// # Errors
/// Refused when `req` carries a config that differs from the Box's.
pub(crate) fn check_kept(cx: &Cx, req: &StartReq, row: &BoxRow) -> Result<(), CommandError> {
    let Some(asked) = override_text(req.config.clone()) else {
        return Ok(());
    };
    if cx.store()?.box_config_override(row.id)?.as_deref() == Some(asked.as_str()) {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "open Box {} has a config of its own, which a start does not change",
            row.name.as_str()
        ),
    )
    .with_hint("replace it with `moor box-config`, or close the Box and start a new one"))
}

/// `box-config-set`: replaces the Box's override with `req.config`, or removes it when that is
/// absent or blank.
///
/// # Errors
/// Failed for a Box that is not open; Usage for text that does not parse; a store failure.
pub(crate) fn set(cx: &Cx, req: &BoxConfigSetReq) -> Result<BoxConfigValue, CommandError> {
    check(req.config.as_deref())?;
    let store = cx.store()?;
    let id = store
        .open_box_by_name(&req.box_name)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open Box {}", req.box_name.as_str()),
            )
        })?
        .id;
    let text = override_text(req.config.clone());
    store.set_box_config_override(id, text.as_deref())?;
    Ok(BoxConfigValue {
        box_name: req.box_name.as_str().to_owned(),
        set: text.is_some(),
    })
}

#[cfg(test)]
mod tests;
