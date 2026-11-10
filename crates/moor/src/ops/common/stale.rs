//! Writes about hands or a launch a Box has since replaced.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::{BoxId, LaunchId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{LaunchEnd, Store, StoreError};

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

/// What `written` wrote; none when it was about hands or a launch the Box has since replaced.
/// Nothing is left to do for what it was about, so this warns once and the caller moves on, never retrying.
pub(crate) fn unless_stale<T>(
    cx: &Cx,
    written: Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    match written {
        Ok(v) => Ok(Some(v)),
        Err(e @ StoreError::Stale { read, .. }) => {
            if let Some(log) = cx.env().shared::<ControllerLog>() {
                log.line(&format!("{}: {e}", read.code()));
            }
            cx.warn(Warning {
                code: read.code().to_owned(),
                message: e.to_string(),
            });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// What ending a launch read earlier came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// It is ended, now or by another op meanwhile.
    Ended,
    /// Another op began a newer launch since the read; nothing was written.
    Replaced,
}

/// Ends `launch` of Box `id` as `end` says. A launch that ended meanwhile with no newer one
/// counts as ended, with nothing more written: new hands end a launch and keep its Session open.
///
/// # Errors
/// A store failure.
pub(crate) fn end_read_launch(
    cx: &Cx,
    store: &Store,
    (id, launch): (BoxId, LaunchId),
    end: &LaunchEnd<'_>,
) -> Result<Ending, CommandError> {
    match store.end_launch(id, launch, end) {
        Ok(()) => Ok(Ending::Ended),
        Err(e @ StoreError::Stale { .. }) => {
            if store.live_launch(id)?.is_none() {
                return Ok(Ending::Ended);
            }
            unless_stale(cx, Err::<(), _>(e))?;
            Ok(Ending::Replaced)
        }
        Err(e) => Err(e.into()),
    }
}
