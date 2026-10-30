//! Crabbox handles and slugs: how a Box is named in calls to crabbox.

use moor_core::box_key::BoxKey;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::AcquireReq;
use moor_core::seam::proc::ProcSpec;

use super::{DEFAULT_SSH_PORT, STATIC_PROVIDER};

/// Per-Box crabbox slug, `moor-{id}-{nonce}`; the Box name never enters it.
pub(super) fn slug(req: &AcquireReq) -> String {
    if super::pool::is_pool_provider(&req.provider) {
        return format!("moor-{}", req.box_id.get());
    }
    BoxKey::new(req.box_id).slug(&req.nonce)
}

/// The handle for a Box named `name` under `provider`.
pub(super) fn handle_of(provider: &str, name: &str) -> BoxHandle {
    BoxHandle::new(format!("{provider}/{name}"))
}

/// Adds the static provider's Box id and port to a call on `handle`.
pub(super) fn static_env(spec: ProcSpec, handle: &BoxHandle) -> ProcSpec {
    match split_handle(handle) {
        (Some(STATIC_PROVIDER), name) => spec
            .env("CRABBOX_STATIC_ID", name)
            .env("CRABBOX_STATIC_PORT", DEFAULT_SSH_PORT.to_string()),
        _ => spec,
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
    let provider = provider.into_iter().flat_map(|p| ["--provider", p]);
    provider.chain(["--id", name]).map(str::to_owned).collect()
}
