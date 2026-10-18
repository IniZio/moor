//! Per-Task interception CA that survives a proxy restart.
//!
//! The Box trusts the certificate for the Task's whole life, so the CA is
//! persisted under the state directory and reloaded rather than regenerated.
//! The private key stays on the host and is never logged or put in an error.

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io::{Error, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::BoxId;
use nono_proxy::config::PreloadedCa;
use nono_proxy::tls_intercept::EphemeralCa;
use zeroize::Zeroizing;

/// File holding the PKCS#8 PEM private key.
const KEY_FILE: &str = "ca-key.pem";
/// File holding the PEM certificate.
const CERT_FILE: &str = "ca.pem";
/// How long a generated CA certificate is valid.
const VALIDITY: Duration = Duration::from_hours(90 * 24);

/// The Task's interception CA. The key never leaves the host and is never
/// shown by `Debug`.
pub(crate) struct TaskCa {
    /// PKCS#8 DER private key, zeroized on drop.
    key_der: Zeroizing<Vec<u8>>,
    /// PEM certificate (public).
    cert_pem: String,
}

impl std::fmt::Debug for TaskCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TaskCa(..)")
    }
}

impl TaskCa {
    /// The PEM certificate the Box trusts.
    pub(crate) fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// The CA as a signer for the GraphQL filter's TLS listener.
    pub(crate) fn ephemeral(&self) -> std::io::Result<EphemeralCa> {
        EphemeralCa::from_existing(&self.key_der, &self.cert_pem)
            .map_err(|_| Error::other("proxy CA is not usable"))
    }

    /// The key and certificate in the form the proxy config takes.
    pub(crate) fn preloaded(&self) -> PreloadedCa {
        PreloadedCa {
            key_der: self.key_der.clone(),
            cert_pem: self.cert_pem.clone(),
        }
    }
}

/// `<state_dir>/proxy/<task>/`, where the Task's CA lives.
pub(crate) fn ca_dir(state_dir: &Path, task: BoxId) -> PathBuf {
    state_dir.join("proxy").join(task.get().to_string())
}

/// Path of the Task's CA certificate, which the Box trusts.
pub(crate) fn cert_path(state_dir: &Path, task: BoxId) -> PathBuf {
    ca_dir(state_dir, task).join(CERT_FILE)
}

/// Loads the Task's CA, or generates and persists one when neither file
/// exists. Directory and files are owner-only; looser existing modes are
/// tightened.
///
/// A half-present or unparsable CA is an error naming the file: the Box
/// trusts the old certificate, so a silent replacement would break it.
pub(crate) fn load_or_create(state_dir: &Path, task: BoxId) -> std::io::Result<TaskCa> {
    let dir = ca_dir(state_dir, task);
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    std::fs::set_permissions(&dir, Permissions::from_mode(0o700))?;
    let key_path = dir.join(KEY_FILE);
    let cert_path = dir.join(CERT_FILE);
    match (key_path.exists(), cert_path.exists()) {
        (false, false) => create(&dir, &key_path, &cert_path, task),
        (true, true) => load(&key_path, &cert_path),
        (true, false) => Err(missing(&cert_path)),
        (false, true) => Err(missing(&key_path)),
    }
}

/// Removes the Task's CA directory and everything in it; Ok if absent.
pub(crate) fn remove(state_dir: &Path, task: BoxId) -> std::io::Result<()> {
    match std::fs::remove_dir_all(ca_dir(state_dir, task)) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Error for a CA file that is absent while its partner exists.
fn missing(path: &Path) -> Error {
    Error::new(
        ErrorKind::NotFound,
        format!("proxy CA file {} is missing", path.display()),
    )
}

/// Error for a CA file that cannot be used; carries no file content.
fn invalid(path: &Path) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("proxy CA file {} is not a valid CA", path.display()),
    )
}

/// Generates a CA and writes cert and key atomically, key first.
fn create(dir: &Path, key_path: &Path, cert_path: &Path, task: BoxId) -> std::io::Result<TaskCa> {
    let cn = format!("moor task {}", task.get());
    let ca = EphemeralCa::generate_with_cn(&cn, VALIDITY)
        .map_err(|_| Error::other("proxy CA generation failed"))?;
    write_private(dir, key_path, ca.key_pem().as_bytes())?;
    write_private(dir, cert_path, ca.cert_pem().as_bytes())?;
    Ok(TaskCa {
        key_der: Zeroizing::new(ca.key_der().to_vec()),
        cert_pem: ca.cert_pem().to_owned(),
    })
}

/// Reads both files and checks that the key matches the certificate.
fn load(key_path: &Path, cert_path: &Path) -> std::io::Result<TaskCa> {
    let key_pem = Zeroizing::new(std::fs::read_to_string(key_path).map_err(|_| invalid(key_path))?);
    let cert_pem = std::fs::read_to_string(cert_path).map_err(|_| invalid(cert_path))?;
    let combined = Zeroizing::new(format!("{}\n{cert_pem}", key_pem.as_str()));
    let (key_der, _) = nono_proxy::tls_intercept::ca::split_key_cert_pem(&combined)
        .map_err(|_| invalid(key_path))?;
    EphemeralCa::from_existing(&key_der, &cert_pem).map_err(|_| invalid(key_path))?;
    std::fs::set_permissions(key_path, Permissions::from_mode(0o600))?;
    std::fs::set_permissions(cert_path, Permissions::from_mode(0o600))?;
    Ok(TaskCa { key_der, cert_pem })
}

/// Writes `bytes` to `path` via a 0600 temp file and a rename, so the file
/// is never world-readable and never half-written.
fn write_private(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("ca")
    ));
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
