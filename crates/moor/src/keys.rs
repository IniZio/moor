//! The ssh key Boxes trust and the front Proxy's CA, sealed in the store. The files under the
//! state dir are a cache of them. A stored value is never replaced, and one the vault cannot
//! open stops the start, as a regenerated key would cut off every Box.

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use moor_core::Secret;

use crate::proxy::fronts::{self, ca_dir};
use crate::proxy::nono::ca;
use crate::store::Store;
use crate::vault::Vault;

/// Store label of the ssh key.
const SSH_LABEL: &str = "ssh-key";
/// Store label of the front CA.
const FRONT_CA_LABEL: &str = "front-proxy-ca";
/// The private key's file in the pool dir.
const SSH_KEY: &str = "id_ed25519";

/// `<state>/pool`, holding the ssh key.
pub(crate) fn pool_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("pool")
}

/// Puts the ssh key, and the front CA when `front_ca` is set, in the store and in files.
pub(crate) fn settle(
    store: &Store,
    vault: &Vault,
    state_dir: &Path,
    front_ca: bool,
) -> Result<(), String> {
    let pool = pool_dir(state_dir);
    let bundle = resolve(store, vault, SSH_LABEL, "ssh key", || import_ssh(&pool))?;
    install_ssh(&pool, &bundle)?;
    if front_ca {
        let dir = ca_dir(state_dir);
        let bundle = resolve(store, vault, FRONT_CA_LABEL, "front Proxy CA", || {
            import_ca(&dir)
        })?;
        install_ca(&dir, &bundle)?;
    }
    Ok(())
}

/// An ssh key in `state_dir`'s pool dir, for a test driver with no store.
#[cfg(test)]
pub(crate) fn scratch_ssh_key(state_dir: &Path) {
    import_ssh(&pool_dir(state_dir)).expect("a scratch ssh key");
}

/// The value under `label`, opened; `import` supplies one only when none is kept.
fn resolve(
    store: &Store,
    vault: &Vault,
    label: &str,
    what: &str,
    import: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Secret, String> {
    let open = |sealed: &[u8]| {
        vault.open_kept(label, sealed).map_err(|e| {
            format!(
                "the vault cannot open the controller's {what}: {e}; \
                 not making a new one, as every Box trusts the stored one"
            )
        })
    };
    let store_err =
        |e: crate::store::StoreError| format!("cannot keep the controller's {what}: {e}");
    if let Some(sealed) = store.sealed(label).map_err(store_err)? {
        return open(&sealed);
    }
    let sealed = vault
        .seal_kept(label, &Secret::new(import()?))
        .map_err(|e| format!("cannot seal the controller's {what}: {e}"))?;
    let kept = store.keep_sealed(label, &sealed).map_err(store_err)?;
    open(&kept)
}

/// The key files in `pool` as one value, made first if absent.
fn import_ssh(pool: &Path) -> Result<Vec<u8>, String> {
    mkdir(pool)?;
    let key = pool.join(SSH_KEY);
    if !key.exists() {
        let made = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "moor-controller",
                "-f",
            ])
            .arg(&key)
            .stdin(Stdio::null())
            .status();
        if !made.is_ok_and(|s| s.success()) {
            return Err("cannot make the controller's ssh key".into());
        }
    }
    let read =
        |path: &Path| fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()));
    let mut bundle = read(&key)?;
    if !bundle.ends_with(b"\n") {
        bundle.push(b'\n');
    }
    bundle.extend_from_slice(&read(&key.with_extension("pub"))?);
    Ok(bundle)
}

/// Writes `bundle` to `pool`, private key 0600 and public key 0644.
fn install_ssh(pool: &Path, bundle: &Secret) -> Result<(), String> {
    let text =
        std::str::from_utf8(bundle.expose()).map_err(|_| "the stored ssh key is not text")?;
    let text = text.trim_end();
    let (private, public) = text
        .rsplit_once('\n')
        .ok_or("the stored ssh key has no public half")?;
    mkdir(pool)?;
    let key = pool.join(SSH_KEY);
    write_file(&key, format!("{private}\n").as_bytes(), 0o600)?;
    write_file(
        &key.with_extension("pub"),
        format!("{public}\n").as_bytes(),
        0o644,
    )
}

/// The CA files in `dir` as one value, made first if absent.
fn import_ca(dir: &Path) -> Result<Vec<u8>, String> {
    ca::load_or_create_in(dir, fronts::CA_NAME)
        .map_err(|e| format!("cannot load the front Proxy CA: {e}"))?;
    let (key, cert) =
        ca::read_in(dir).map_err(|e| format!("cannot read the front Proxy CA: {e}"))?;
    let mut bundle = key.to_vec();
    if !bundle.ends_with(b"\n") {
        bundle.push(b'\n');
    }
    bundle.extend_from_slice(cert.as_bytes());
    Ok(bundle)
}

/// Writes `bundle` to `dir`.
fn install_ca(dir: &Path, bundle: &Secret) -> Result<(), String> {
    let text = std::str::from_utf8(bundle.expose()).map_err(|_| "the stored CA is not text")?;
    let at = text
        .find("-----BEGIN CERTIFICATE-----")
        .ok_or("the stored CA has no certificate")?;
    let (key, cert) = text.split_at(at);
    ca::install_in(dir, (key.as_bytes(), cert))
        .map(drop)
        .map_err(|e| format!("cannot write the front Proxy CA: {e}"))
}

/// Makes `dir` owner-only.
fn mkdir(dir: &Path) -> Result<(), String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .and_then(|()| fs::set_permissions(dir, Permissions::from_mode(0o700)))
        .map_err(|e| format!("cannot make {}: {e}", dir.display()))
}

/// Writes `path` through a temp file and a rename.
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let wrote = (|| {
        match fs::remove_file(&tmp) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    wrote.map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
