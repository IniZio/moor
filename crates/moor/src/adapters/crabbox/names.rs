//! Box names, handles and selector arguments.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{AcquireReq, DriverError};

/// Per-Task Box name: `moor-{name}-{nonce}`.
pub(super) fn box_name(req: &AcquireReq) -> String {
    format!("moor-{}-{}", req.name.as_str(), req.nonce)
}

/// The handle for a Box named `name` under `provider`.
pub(super) fn handle_of(provider: &str, name: &str) -> BoxHandle {
    BoxHandle::new(format!("{provider}/{name}"))
}

/// Maps a moor size to crabbox's machine class.
pub(super) fn size_of(size: Option<&str>) -> Result<&'static str, DriverError> {
    match size.unwrap_or("small") {
        "small" => Ok("standard"),
        "medium" => Ok("fast"),
        "large" => Ok("large"),
        "xlarge" => Ok("beast"),
        other => Err(DriverError::Failed {
            summary: format!("unknown box size {other:?}"),
        }),
    }
}

/// Splits a handle into optional provider and Box name.
pub(super) fn split_handle(handle: &BoxHandle) -> (Option<&str>, &str) {
    match handle.as_str().split_once('/') {
        Some((p, n)) if !p.is_empty() => (Some(p), n),
        Some((_, n)) => (None, n),
        None => (None, handle.as_str()),
    }
}

/// Selector arguments that name one Box.
pub(super) fn target_args(handle: &BoxHandle) -> Vec<String> {
    let (provider, name) = split_handle(handle);
    let mut args = Vec::new();
    if let Some(p) = provider {
        args.extend(["--provider".to_owned(), p.to_owned()]);
    }
    args.extend(["--id".to_owned(), name.to_owned()]);
    args
}

/// Error for output moor cannot read.
pub(super) fn unreadable(what: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("unreadable box provider output for {what}"),
    }
}
