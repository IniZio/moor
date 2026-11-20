//! `github-app`: the operator's GitHub App, as the portal shows it.

use crate::adapters::github::app::{AppClient, Installed};
use crate::cx::Cx;
use moor_core::ErrorKind;

use crate::error::CommandError;

/// The App's install page and the repos it is installed on; none when the controller config
/// names no App.
///
/// # Errors
/// GitHub cannot be reached or refuses a read.
pub(crate) fn installed(cx: &Cx) -> Result<Option<Installed>, CommandError> {
    let Some(app) = cx.env().shared::<AppClient>() else {
        return Ok(None);
    };
    app.installed(cx.clock().now())
        .map(Some)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))
}
