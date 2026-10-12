//! `moor version`.

/// Envelope schema version reported by `moor version`.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Payload of `moor version`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct VersionView {
    /// Crate version.
    pub(crate) version: &'static str,
    /// Git commit the binary was built from.
    pub(crate) commit: &'static str,
    /// Envelope schema version.
    pub(crate) schema: u32,
}

/// Build the version payload.
pub(crate) const fn exec() -> VersionView {
    VersionView {
        version: env!("CARGO_PKG_VERSION"),
        commit: env!("MOOR_COMMIT"),
        schema: SCHEMA_VERSION,
    }
}
