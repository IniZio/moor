//! The proxy spec file the host writes and `moor proxy serve` reads.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use moor_core::seam::proxy_host::ProxySpec;
use moor_core::BoxId;

use crate::proxy::nono::ca::ca_dir;

/// Name of the spec file inside the Task's proxy directory.
const SPEC_FILE: &str = "spec.json";

/// `<state_dir>/proxy/<task>/spec.json`.
pub(crate) fn spec_path(state_dir: &Path, task: BoxId) -> PathBuf {
    ca_dir(state_dir, task).join(SPEC_FILE)
}

/// Write the spec owner-only: a temporary file in the same directory, then a rename.
pub(crate) fn write_spec(state_dir: &Path, spec: &ProxySpec) -> io::Result<()> {
    let path = spec_path(state_dir, spec.box_id);
    let dir = ca_dir(state_dir, spec.box_id);
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    fs::set_permissions(&dir, Permissions::from_mode(0o700))?;
    let bytes = serde_json::to_vec(spec).map_err(io::Error::from)?;
    let tmp = dir.join(format!("{SPEC_FILE}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    fs::set_permissions(&tmp, Permissions::from_mode(0o600))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, &path)
}

/// Read the spec the host wrote for `task`.
pub(crate) fn read_spec(state_dir: &Path, task: BoxId) -> io::Result<ProxySpec> {
    let bytes = fs::read(spec_path(state_dir, task))?;
    serde_json::from_slice(&bytes).map_err(io::Error::from)
}
