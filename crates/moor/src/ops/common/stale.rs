//! Writes about hands a Box has since replaced.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::Warning;

use crate::cx::Cx;
use crate::store::StoreError;

/// The controller's log, shared through the env so a dropped write reaches it as well as the caller.
pub(crate) struct ControllerLog(Arc<Mutex<dyn Write + Send>>);

impl ControllerLog {
    /// A handle writing to `sink`.
    pub(crate) fn new(sink: &Arc<Mutex<dyn Write + Send>>) -> Self {
        Self(Arc::clone(sink))
    }

    /// Writes one `moor controller: <msg>` line.
    pub(crate) fn line(&self, msg: &str) {
        drop(writeln!(
            self.0.lock().unwrap_or_else(PoisonError::into_inner),
            "moor controller: {msg}"
        ));
    }
}

/// What `written` wrote; none when it was about hands the Box has since replaced. Nothing is left
/// to do for the VM it was about, so this warns once and the caller moves on, never retrying.
pub(crate) fn unless_stale<T>(
    cx: &Cx,
    written: Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    match written {
        Ok(v) => Ok(Some(v)),
        Err(e @ StoreError::Stale { .. }) => {
            if let Some(log) = cx.env().shared::<ControllerLog>() {
                log.line(&format!("stale_hands_write: {e}"));
            }
            cx.warn(Warning {
                code: "stale_hands_write".to_owned(),
                message: e.to_string(),
            });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
