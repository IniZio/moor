//! Helpers for acquiring a Box: size mapping and the box-shape guard.

use moor_core::config::BoxShape;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::ProcSpec;

use super::{split_handle, DEFAULT_SSH_PORT, STATIC_PROVIDER};

#[cfg(test)]
mod tests;

/// Provider ids (and aliases) crabbox routes to the external provider.
const EXTERNAL_PROVIDERS: [&str; 2] = ["external", "exec-provider"];

/// Adds the static provider's Box id and port to a call on `handle`.
pub(super) fn static_env(spec: ProcSpec, handle: &BoxHandle) -> ProcSpec {
    match split_handle(handle) {
        (Some(STATIC_PROVIDER), name) => spec
            .env("CRABBOX_STATIC_ID", name)
            .env("CRABBOX_STATIC_PORT", DEFAULT_SSH_PORT.to_string()),
        _ => spec,
    }
}

/// Passes the box shape to the external provider's adapter as environment.
///
/// Crabbox forwards the environment of a warmup to the adapter.
pub(super) fn shape_env(mut spec: ProcSpec, shape: &BoxShape) -> ProcSpec {
    if let Some(template) = &shape.template {
        spec = spec.env("CRABBOX_AGENT37_TEMPLATE", template);
    }
    if let Some(gb) = shape.disk_gb {
        spec = spec.env("CRABBOX_AGENT37_DISK_GB", gb.to_string());
    }
    spec
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

/// Refuses a box shape on a provider that cannot take it.
///
/// Ignoring the keys silently would hand back a Box without the expected
/// toolchain or disk, failing later and far from the cause.
pub(super) fn check_shape(provider: &str, shape: &BoxShape) -> Result<(), DriverError> {
    if shape.is_empty() || EXTERNAL_PROVIDERS.contains(&provider) {
        return Ok(());
    }
    let mut keys = Vec::new();
    if shape.template.is_some() {
        keys.push("box.template");
    }
    if shape.disk_gb.is_some() {
        keys.push("box.disk_gb");
    }
    Err(DriverError::Failed {
        summary: format!(
            "`.moor.yaml` sets {}, but provider `{provider}` cannot take them; only the external provider (crabbox-agent37) reads a template and disk size. Remove them or use the external provider.",
            keys.join(", ")
        ),
    })
}
